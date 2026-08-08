use color_eyre::eyre::Result;

pub struct WifiScanner {}

impl WifiScanner {}

/*
let scan_thread_run = Arc::new(AtomicBool::new(false));
let scan_thread: tokio::task::JoinHandle<Result<(), wifi_scan::Error>> = tokio::spawn({
     let str = scan_thread_run.clone();
     async move {
         while str.load(Relaxed) {
             let r = wifi_scan::scan()?;
             println!("---------------");
             r.into_iter()
                 // .map(|e| (e.ssid, e.mac, e.signal_level))
                 .for_each(|e| println!("{e:?}"));
             tokio::time::sleep(Duration::from_millis(1000)).await;
         }
         Ok(())
     }
 });
 */
