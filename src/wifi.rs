//! Fast single-AP signal sampling.
//!
//! A full area scan sweeps the radio across every channel, which is slow. For the AP the adapter
//! is *associated* with, the driver already keeps a live RSSI value that can be read without
//! touching the radio, which is what [`WlanSession`] exposes.

/// The AP the adapter is currently associated with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedAp {
    pub ssid: String,
    /// Upper-case, colon separated: `"AA:BB:CC:DD:EE:FF"`.
    pub mac: String,
}

#[cfg(target_os = "windows")]
mod imp {
    use std::ptr;

    use windows_sys::Win32::NetworkManagement::WiFi::{
        WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WLAN_OPCODE_VALUE_TYPE,
        WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle, WlanQueryInterface,
        wlan_interface_state_connected, wlan_intf_opcode_current_connection, wlan_intf_opcode_rssi,
    };
    use windows_sys::core::GUID;

    use super::ConnectedAp;

    /// `wlan_intf_opcode_rssi` is undocumented and not portable across drivers: an Intel 8265
    /// answers with an SNR-like `+32` while the link is at -63 dBm. A connected AP's RSSI is always
    /// negative, so anything outside this range means the opcode is unusable on this adapter.
    const PLAUSIBLE_DBM: std::ops::RangeInclusive<i32> = -100..=-10;

    /// Turn a Win32 error code into a `Result` so failures surface as messages instead of `None`.
    fn ok(code: u32, what: &str) -> Result<(), String> {
        match code {
            0 => Ok(()),
            e => Err(format!("{what} failed: error {e}")),
        }
    }

    pub struct WlanSession {
        handle: isize,
        guid: GUID,
    }

    // The WLAN handle is an opaque kernel handle and wlanapi is thread safe, so the session can be
    // moved to the sampler thread.
    unsafe impl Send for WlanSession {}

    impl WlanSession {
        pub fn open() -> Result<Self, String> {
            unsafe {
                let mut negotiated_version = 0u32;
                let mut handle: isize = 0;

                // Version 2 is what every supported Windows speaks; keep the fallback for drivers
                // that only negotiate version 1.
                if WlanOpenHandle(2, ptr::null(), &mut negotiated_version, &mut handle) != 0 {
                    ok(
                        WlanOpenHandle(1, ptr::null(), &mut negotiated_version, &mut handle),
                        "WlanOpenHandle",
                    )?;
                }

                let mut interface_list: *mut WLAN_INTERFACE_INFO_LIST = ptr::null_mut();
                if let Err(e) = ok(
                    WlanEnumInterfaces(handle, ptr::null(), &mut interface_list),
                    "WlanEnumInterfaces",
                ) {
                    WlanCloseHandle(handle, ptr::null());
                    return Err(e);
                }

                if (*interface_list).dwNumberOfItems == 0 {
                    WlanFreeMemory(interface_list.cast());
                    WlanCloseHandle(handle, ptr::null());
                    return Err("no WLAN interface found".to_string());
                }

                let guid = (*interface_list).InterfaceInfo[0].InterfaceGuid;
                WlanFreeMemory(interface_list.cast());

                Ok(Self { handle, guid })
            }
        }

        /// Read `wlan_intf_opcode_current_connection` and hand the attributes to `f`.
        ///
        /// The buffer is owned by wlanapi, so it is freed before returning and `f` must copy out
        /// anything it wants to keep.
        fn with_connection<T>(
            &self,
            f: impl FnOnce(&WLAN_CONNECTION_ATTRIBUTES) -> T,
        ) -> Result<T, String> {
            unsafe {
                let mut size = 0u32;
                let mut data: *mut core::ffi::c_void = ptr::null_mut();
                let mut value_type: WLAN_OPCODE_VALUE_TYPE = 0;
                ok(
                    WlanQueryInterface(
                        self.handle,
                        &self.guid,
                        wlan_intf_opcode_current_connection,
                        ptr::null(),
                        &mut size,
                        &mut data,
                        &mut value_type,
                    ),
                    "WlanQueryInterface(current_connection)",
                )?;
                if data.is_null() || (size as usize) < size_of::<WLAN_CONNECTION_ATTRIBUTES>() {
                    if !data.is_null() {
                        WlanFreeMemory(data);
                    }
                    return Err("current_connection returned a short buffer".to_string());
                }

                let result = f(&*data.cast::<WLAN_CONNECTION_ATTRIBUTES>());
                WlanFreeMemory(data);
                Ok(result)
            }
        }

        /// Which AP the adapter is associated with right now, if any.
        pub fn connected_ap(&self) -> Option<ConnectedAp> {
            self.with_connection(|attrs| {
                if attrs.isState != wlan_interface_state_connected {
                    return None;
                }
                let assoc = &attrs.wlanAssociationAttributes;
                let len = (assoc.dot11Ssid.uSSIDLength as usize).min(assoc.dot11Ssid.ucSSID.len());
                let ssid = String::from_utf8_lossy(&assoc.dot11Ssid.ucSSID[..len]).into_owned();
                let b = assoc.dot11Bssid;
                Some(ConnectedAp {
                    ssid,
                    mac: format!(
                        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                        b[0], b[1], b[2], b[3], b[4], b[5]
                    ),
                })
            })
            .ok()
            .flatten()
        }

        /// Current RSSI in dBm for the associated AP. Cheap; safe to call at 20 Hz.
        pub fn rssi(&self) -> Option<i32> {
            unsafe {
                let mut size = 0u32;
                let mut data: *mut core::ffi::c_void = ptr::null_mut();
                let mut value_type: WLAN_OPCODE_VALUE_TYPE = 0;
                let code = WlanQueryInterface(
                    self.handle,
                    &self.guid,
                    wlan_intf_opcode_rssi,
                    ptr::null(),
                    &mut size,
                    &mut data,
                    &mut value_type,
                );
                if code == 0 && !data.is_null() && size as usize >= size_of::<i32>() {
                    let value = data.cast::<i32>().read_unaligned();
                    WlanFreeMemory(data);
                    if PLAUSIBLE_DBM.contains(&value) {
                        return Some(value);
                    }
                } else if !data.is_null() {
                    WlanFreeMemory(data);
                }
            }

            // Either the driver rejected the opcode or it answered with something that is not dBm.
            // Signal quality is a documented linear scale over -100..-50 dBm, so it recovers the
            // value quantised to 2 dBm.
            self.with_connection(|attrs| {
                if attrs.isState != wlan_interface_state_connected {
                    return None;
                }
                Some(attrs.wlanAssociationAttributes.wlanSignalQuality as i32 / 2 - 100)
            })
            .ok()
            .flatten()
        }
    }

    impl Drop for WlanSession {
        fn drop(&mut self) {
            unsafe {
                WlanCloseHandle(self.handle, ptr::null());
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::process::Command;

    use super::ConnectedAp;

    pub struct WlanSession {
        interface: String,
    }

    /// First data line of `/proc/net/wireless` (two header lines precede it), as
    /// `(interface, level_dbm)`.
    fn proc_net_wireless() -> Option<(String, i32)> {
        let contents = std::fs::read_to_string("/proc/net/wireless").ok()?;
        let line = contents.lines().nth(2)?;
        let (name, rest) = line.split_once(':')?;
        // Columns: status, link, level, noise, ... The values carry a trailing '.'.
        let level = rest.split_whitespace().nth(2)?.trim_end_matches('.');
        Some((name.trim().to_string(), level.parse::<f32>().ok()? as i32))
    }

    impl WlanSession {
        pub fn open() -> Result<Self, String> {
            let (interface, _) = proc_net_wireless()
                .ok_or_else(|| "no wireless interface in /proc/net/wireless".to_string())?;
            Ok(Self { interface })
        }

        /// Which AP the adapter is associated with right now, if any.
        pub fn connected_ap(&self) -> Option<ConnectedAp> {
            let output = Command::new("iw")
                .args(["dev", &self.interface, "link"])
                .output()
                .ok()?;
            let stdout = String::from_utf8_lossy(&output.stdout);

            let mut mac = None;
            let mut ssid = None;
            for line in stdout.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("Connected to ") {
                    mac = rest.split_whitespace().next().map(|m| m.to_uppercase());
                } else if let Some(rest) = line.strip_prefix("SSID: ") {
                    // Take the rest of the line: SSIDs may contain spaces.
                    ssid = Some(rest.to_string());
                }
            }
            Some(ConnectedAp {
                ssid: ssid?,
                mac: mac?,
            })
        }

        /// Current RSSI in dBm for the associated AP. A file read, so 20 Hz is free.
        pub fn rssi(&self) -> Option<i32> {
            proc_net_wireless().map(|(_, level)| level)
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use super::ConnectedAp;

    /// No fast path on this platform; the UI reports `Unsupported`.
    pub struct WlanSession;

    impl WlanSession {
        pub fn open() -> Result<Self, String> {
            Ok(Self)
        }

        pub fn connected_ap(&self) -> Option<ConnectedAp> {
            None
        }

        pub fn rssi(&self) -> Option<i32> {
            None
        }
    }
}

/// A WLAN handle held open for the lifetime of the app, so a sample costs nothing but the query.
pub use imp::WlanSession;

#[cfg(test)]
mod test {
    use super::*;

    /// Needs a real adapter, so it stays out of the normal run:
    /// `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_session() {
        let session = WlanSession::open().expect("open WLAN session");
        println!("connected_ap: {:?}", session.connected_ap());
        println!("rssi: {:?} dBm", session.rssi());
    }
}
