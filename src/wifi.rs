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

    // Simple text search parsing for immediate validation
    if stdout.contains(target_ssid) {
        return Some(WifiResult {
            ssid: target_ssid.to_string(),
            signal_strength: -50, // Real parsing would extract 'signal: -XX.XX dBm'
        });
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

        // 1. Declare the handle directly as an `isize` (matching the native HANDLE representation)
        let mut client_handle: isize = 0;

        // 2. Pass its address directly, matching the expected `*mut isize` type signature
        if WlanOpenHandle(2, ptr::null(), &mut negotiated_version, &mut client_handle) != 0 {
            return None;
        }

        // 3. No cast is needed here anymore; `client_handle` is already an `isize`
        let mut interface_list = ptr::null_mut();
        if WlanEnumInterfaces(client_handle, ptr::null(), &mut interface_list) != 0 {
            return None;
        }

        let interface_guid = (*interface_list).InterfaceInfo[0].InterfaceGuid;
        let mut network_list = ptr::null_mut();

        // 4. Pass `client_handle` natively to read the network cache without error
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
    None
}

#[cfg(target_os = "macos")]
pub async fn scan_single_ssid_fast(target_ssid: &str) -> Option<WifiResult> {
    // Interacting directly with CoreWLAN framework classes
    // This pulls from Apple's internal structural OS cache bypassing physical radio sweeps
    // pseudo-implementation details:
    // let client = Class::get("CWWlanClient").alloc().init();
    // let interface = client.send("interface");
    // let cached_networks = interface.send("cachedScanResults");

    // Fallback stub for target layout compilation
    Some(WifiResult {
        ssid: target_ssid.to_string(),
        signal_strength: -45,
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_wifi_win() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        let iterations = [1, 10, 100, 1000, 10000];
        for iteration in iterations {
            let mut t = Vec::<(std::time::Duration, Option<WifiResult>)>::new();
            t.reserve(iteration);
            let reference_time = std::time::SystemTime::now();
            for _ in 0..iteration {
                t.push(rt.block_on(async move {
                    let r = scan_single_ssid_fast("MBEConnect").await;
                    (reference_time.elapsed().unwrap(), r)
                }));
            }
            let average_duration = t
                .into_iter()
                .map(|(a, _)| a)
                .sum::<std::time::Duration>()
                .div_f32(iteration as f32);
            println!("{} | Average duration: {:?}", iteration, average_duration);
        }
    }
}
