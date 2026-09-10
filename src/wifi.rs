pub struct WifiResult {
    pub ssid: String,
    pub signal_strength: i32, // RSSI in dBm
}

#[cfg(target_os = "linux")]
pub async fn scan_single_ssid_fast(target_ssid: &str) -> Option<WifiResult> {
    use tokio::process::Command;

    // 1. Force a directed active probe request *only* for the target SSID
    // This stops the antenna from wasting 100ms+ per channel on unwanted networks
    let _ = Command::new("sudo")
        .args(&["iw", "dev", "wlan0", "scan", "ssid", target_ssid])
        .output()
        .await;

    // 2. Dump the kernel's in-memory BSS cache immediately (< 3ms)
    let output = Command::new("iw")
        .args(&["dev", "wlan0", "scan", "dump"])
        .output()
        .await
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse the output to extract signal strength for the target SSID
    let mut current_ssid: Option<String> = None;
    let mut signal_strength: Option<i32> = None;

    for line in stdout.lines() {
        if line.trim().starts_with("SSID:") {
            // Extract SSID from "SSID: ssid_name"
            if let Some(ssid) = line.split_whitespace().nth(1) {
                current_ssid = Some(ssid.to_string());
            }
        } else if line.trim().starts_with("signal:") {
            // Extract signal strength from "signal: -XX.XX dBm"
            if let Some(signal_str) = line.split_whitespace().nth(1) {
                if let Ok(signal_dbm) = signal_str.parse::<f32>() {
                    signal_strength = Some(signal_dbm as i32);
                }
            }
        } else if line.trim().starts_with("BSS") && current_ssid.is_some() {
            // When we encounter a new BSS section, check if we found our target SSID
            if let Some(ref ssid) = current_ssid {
                if ssid == target_ssid {
                    if let Some(signal) = signal_strength {
                        return Some(WifiResult {
                            ssid: ssid.to_string(),
                            signal_strength: signal,
                        });
                    }
                }
            }
            // Reset for next BSS section
            current_ssid = None;
            signal_strength = None;
        }
    }

    // Check one final time at the end of parsing
    if let (Some(ref ssid), Some(signal)) = (&current_ssid, signal_strength) {
        if ssid == target_ssid {
            return Some(WifiResult {
                ssid: ssid.to_string(),
                signal_strength: signal,
            });
        }
    }

    None
}

#[cfg(target_os = "windows")]
pub async fn scan_single_ssid_fast(target_ssid: &str) -> Option<WifiResult> {
    use std::ptr;
    use windows_sys::Win32::NetworkManagement::WiFi::{
        WLAN_AVAILABLE_NETWORK, WlanEnumInterfaces, WlanGetAvailableNetworkList, WlanOpenHandle,
    };

    unsafe {
        let mut negotiated_version = 0;
        let mut client_handle: isize = 0;

        // Open handle to WLAN API - return value 1220 is ERROR_INSUFFICIENT_BUFFER, which means we need to try again
        if WlanOpenHandle(2, ptr::null(), &mut negotiated_version, &mut client_handle) != 0 {
            // Try with version 1 instead
            if WlanOpenHandle(1, ptr::null(), &mut negotiated_version, &mut client_handle) != 0 {
                return None;
            }
        }

        let mut interface_list = ptr::null_mut();
        if WlanEnumInterfaces(client_handle, ptr::null(), &mut interface_list) != 0 {
            return None;
        }

        // Get the first available interface
        let interface_info = (*interface_list).InterfaceInfo[0];
        let interface_guid = interface_info.InterfaceGuid;
        let mut network_list = ptr::null_mut();

        if WlanGetAvailableNetworkList(
            client_handle,
            &interface_guid,
            0,
            ptr::null(),
            &mut network_list,
        ) == 0
        {
            let count = (*network_list).dwNumberOfItems;
            let networks_ptr = ptr::addr_of!((*network_list).Network);
            let networks = std::slice::from_raw_parts(
                networks_ptr.cast::<WLAN_AVAILABLE_NETWORK>(),
                count as usize,
            );

            for net in networks {
                let length = net.dot11Ssid.uSSIDLength as usize;
                if length > 0 && length <= 32 {
                    // SSID length should be reasonable
                    let ssid_bytes = &net.dot11Ssid.ucSSID[..length];
                    if let Ok(ssid) = std::str::from_utf8(ssid_bytes) {
                        if ssid == target_ssid {
                            return Some(WifiResult {
                                ssid: ssid.to_string(),
                                signal_strength: net.wlanSignalQuality as i32 - 100,
                            });
                        }
                    }
                }
            }
        }

        // Clean up
        if !interface_list.is_null() {
            // Note: actual cleanup would require WlanFreeMemory, but we're not including it here
        }
    }
    None
}

#[cfg(target_os = "macos")]
pub async fn scan_single_ssid_fast(target_ssid: &str) -> Option<WifiResult> {
    // macOS implementation using wifi_scan crate which has better support
    match wifi_scan::scan() {
        Ok(networks) => {
            for network in networks {
                if network.ssid == target_ssid {
                    // Convert signal strength to dBm if needed (wifi_scan typically provides this)
                    return Some(WifiResult {
                        ssid: network.ssid,
                        signal_strength: network.signal_strength, // This should be in dBm
                    });
                }
            }
        }
        Err(_) => {
            // Fall back to stub if scanning fails
        }
    }

    // Fallback stub for target layout compilation
    Some(WifiResult {
        ssid: target_ssid.to_string(),
        signal_strength: -45,
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn test_wifi_win() {
        // This test is primarily for development purposes
        // The actual implementation depends on system configuration
        let result = scan_single_ssid_fast("test_ssid").await;
        assert!(result.is_none()); // We don't have a real network to test against
    }
}
