use btleplug::api::bleuuid::BleUuid;
use futures::StreamExt;
use rand::seq::SliceRandom;
use rand::Rng;
use std::error::Error;
use std::time::Duration;
use tokio::time::{self, timeout};

use btleplug::api::{Central, CentralEvent, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::{Adapter, Manager};
use uuid::Uuid;

mod font;
use font::get_char_data;

#[derive(Debug, Clone, Copy, Default)]
#[repr(u8)]
enum Mode {
    #[default]
    ScrollLeft = 0,
    ScrollRight = 1,
    ScrollUp = 2,
    ScrollDown = 3,
    Fixed = 4,
    Animation = 5,
    Snowflake = 6,
    Picture = 7,
    Laser = 8,
}

#[derive(Debug, Clone, Default)]
struct Bitmap {
    flash: bool,
    marquee: bool,
    mode: Mode,
    speed: u8,
    data: Vec<u8>,
}

impl Bitmap {
    fn new() -> Self {
        Self::default()
    }

    fn put_string(&mut self, s: &str) {
        self.data.clear();

        for c in s.chars() {
            let char_data = get_char_data(c);

            // Create an 11-row chunk for this character
            let mut char_rows = vec![0u8; 11];

            // Copy the 7 rows of character data, centered vertically
            for (row_idx, &char_row) in char_data.iter().enumerate() {
                // Place character with 2 rows padding at top (11-7)/2 = 2
                char_rows[row_idx + 2] = char_row;
            }

            // Add this character's rows to the bitmap data
            self.data.extend_from_slice(&char_rows);
        }
    }
}

struct Data {
    bitmaps: Vec<Bitmap>,
}

impl Data {
    fn new() -> Self {
        Self { bitmaps: vec![] }
    }

    fn push_bitmap(&mut self, bitmap: Bitmap) {
        self.bitmaps.push(bitmap);
    }

    fn to_bytes(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        let mut data: Vec<u8> = vec![];
        data.extend(b"wang\0\0");

        let mut flash = 0u8;
        let mut marquee = 0u8;
        let mut modes = [0u8; 8];
        let mut sizes = [0u16; 8];

        for (i, bitmap) in self.bitmaps.iter().enumerate() {
            if bitmap.flash {
                flash |= 1 << i;
            }
            if bitmap.marquee {
                marquee |= 1 << i;
            }
            modes[i] = bitmap.speed << 4 | (bitmap.mode as u8);
            sizes[i] = bitmap.data.chunks_exact(11).count() as u16;
        }

        data.push(flash);
        data.push(marquee);
        data.extend(modes);
        data.extend(sizes.iter().flat_map(|size| size.to_be_bytes()));
        // padding
        data.extend(&[0; 6]);
        // timestamp - purpose unclear
        data.extend(&[0; 6]);
        // padding
        data.extend(&[0; 4]);
        // separator
        data.extend(&[0; 16]);

        let mut data_bytes = 0u8;

        for bitmap_data in self.bitmaps.iter().map(|bitmap| &bitmap.data) {
            for chunk in bitmap_data.chunks_exact(11) {
                data.extend(chunk);
                let new_data_bytes = data_bytes.checked_add(11);
                if new_data_bytes.is_none() {
                    continue;
                }
                data_bytes = new_data_bytes.unwrap();
            }
        }

        let padding = data_bytes % 16;
        if padding != 0 {
            data.extend(&[0].repeat(padding as usize));
        }

        Ok(data)
    }
}

async fn get_central(manager: &Manager) -> Adapter {
    let adapters = manager.adapters().await.unwrap();
    adapters.into_iter().next().unwrap()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    pretty_env_logger::init();

    let manager = Manager::new().await?;

    let central = get_central(&manager).await;

    let central_state = central.adapter_state().await.unwrap();
    println!("CentralState: {:?}", central_state);

    let mut events = central.events().await?;

    let mut scan_filter = ScanFilter::default();
    let service = Uuid::from_u128(0x0000fee000001000800000805f9b34fb);
    scan_filter.services.push(service);

    central.start_scan(scan_filter).await?;

    while let Some(event) = events.next().await {
        if let CentralEvent::DeviceDiscovered(device) = event {
            println!("DeviceDiscovered: {:?}", device);
            let peripheral = central.peripheral(&device).await?;
            let properties = peripheral.properties().await?;
            let local_name = properties
                .unwrap()
                .local_name
                .unwrap_or(String::from("(peripheral name unknown)"));
            if local_name != "LSLED" {
                continue;
            }
            println!("Found {}", local_name);
            if !peripheral.is_connected().await? {
                println!("Connecting to peripheral {:?}...", &local_name);
                peripheral.connect().await?;
            }
            peripheral.discover_services().await?;

            for service in peripheral.services() {
                println!("Checking Service: {:?}", service);

                if service.uuid != Uuid::from_u128(0x0000fee000001000800000805f9b34fb) {
                    continue;
                }

                println!(
                    "Service UUID {}, primary: {}",
                    service.uuid, service.primary
                );
                for characteristic in service.characteristics {
                    println!("  {:?}", characteristic);
                    if characteristic.uuid != Uuid::from_u128(0x0000fee100001000800000805f9b34fb) {
                        println!("Skipping characteristic {:?}", characteristic);
                        continue;
                    }

                    println!("Writing to characteristic {:?}", characteristic.uuid);

                    let mut bitmap = Bitmap {
                        flash: false,
                        marquee: false,
                        mode: Mode::Fixed,
                        speed: 5,
                        data: vec![],
                    };

                    let strings = [
                        "ARAFEDD",
                        // "1312",
                        // "FCK AFD",
                        // "I USE ARCH BTW",
                        // "PWND",
                        // "ALL YOUR BASE ARE BELONG TO US",
                        // "ZIVILBULLE",
                    ];
                    let string = strings.choose(&mut rand::thread_rng()).unwrap();
                    bitmap.put_string(string);

                    let data = Data {
                        bitmaps: vec![bitmap],
                    };

                    let data_bytes = data.to_bytes()?;
                    println!("{} total chunks", data_bytes.chunks_exact(16).count());
                    for (i, chunk) in data_bytes.chunks_exact(16).enumerate() {
                        time::sleep(Duration::from_micros(10)).await;
                        if peripheral
                            .write(
                                &characteristic,
                                chunk,
                                btleplug::api::WriteType::WithoutResponse,
                            )
                            .await
                            .is_err()
                        {
                            println!("Error writing chunk {} of {}", i, data_bytes.len());
                            continue;
                        }
                        println!("Wrote chunk {} of {}", i, data_bytes.len());
                    }

                    println!("Done writing to characteristic {:?}", characteristic);

                    let _ = peripheral.disconnect().await;
                }
            }
        }

        std::process::exit(0);
    }

    Ok(())

    // loop {
    //     for adapter in adapter_list.iter() {
    //         println!("Starting scan on {}...", adapter.adapter_info().await?);
    //         let mut scan_filter = ScanFilter::default();
    //         let service = Uuid::from_u128(0x0000fee000001000800000805f9b34fb);
    //         scan_filter.services.push(service);
    //         if timeout(Duration::from_secs(10), adapter.start_scan(scan_filter))
    //             .await
    //             .map(|inner| inner.is_err())
    //             .unwrap_or(true)
    //         {
    //             continue;
    //         }
    //         let peripherals = adapter.peripherals().await?;
    //         if peripherals.is_empty() {
    //             eprintln!("->>> BLE peripheral devices were not found");
    //         } else {
    //             for peripheral in peripherals.iter() {
    //                 let properties = peripheral.properties().await?;
    //                 let is_connected = peripheral.is_connected().await?;
    //                 let local_name = properties
    //                     .unwrap()
    //                     .local_name
    //                     .unwrap_or(String::from("(peripheral name unknown)"));
    //                 if local_name != "LSLED" {
    //                     continue;
    //                 }
    //                 println!(
    //                     "Peripheral {:?} is connected: {:?}",
    //                     local_name, is_connected
    //                 );
    //                 if !is_connected {
    //                     println!("Connecting to peripheral {:?}...", &local_name);
    //                     if let Err(err) = peripheral.connect().await {
    //                         eprintln!("Error connecting to peripheral, skipping: {}", err);
    //                         continue;
    //                     }
    //                 }
    //                 let is_connected = peripheral.is_connected().await?;
    //                 println!(
    //                     "Now connected ({:?}) to peripheral {:?}...",
    //                     is_connected, &local_name
    //                 );
    //                 peripheral.discover_services().await?;
    //                 println!("Discover peripheral {:?} services...", &local_name);
    //                 for service in peripheral.services() {
    //                 }
    //                 if is_connected {
    //                     println!("Disconnecting from peripheral {:?}...", &local_name);
    //                     let _ = peripheral.disconnect().await;
    //                 }
    //             }
    //         }
    //     }
    // }
}
