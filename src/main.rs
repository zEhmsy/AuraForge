use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::time::Duration;

use auraforge::config::{self, Config};
use auraforge::{capture, engine::Engine, govee};

fn main() -> ExitCode {
    let config = match Config::from_args(std::env::args().skip(1)) {
        Ok(Some(config)) => config,
        Ok(None) => {
            print!("{}", config::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{e}\n\n{}", config::USAGE);
            return ExitCode::FAILURE;
        }
    };

    if config.list_monitors {
        return match capture::monitors() {
            Ok(monitors) => {
                for (i, m) in monitors.iter().enumerate() {
                    println!("{i}: {m}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{}", e.message());
                ExitCode::FAILURE
            }
        };
    }

    if config.govee_scan {
        return match govee::scan(Duration::from_secs(3)) {
            Ok(devices) if devices.is_empty() => {
                eprintln!("no Govee strip answered; is \"LAN Control\" enabled in Govee Home?");
                ExitCode::FAILURE
            }
            Ok(devices) => {
                for d in devices {
                    println!("{}  {}  {}", d.ip, d.sku, d.id);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("Govee scan failed: {e}");
                ExitCode::FAILURE
            }
        };
    }

    let engine = Engine::start(config);
    let stop = engine.stop_flag();
    if let Err(e) = ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed)) {
        eprintln!("[auraforge] cannot install Ctrl+C handler: {e}");
    }
    engine.wait();
    ExitCode::SUCCESS
}
