mod config {
    use anyhow::Context;
    use esp32_nimble::BLEAddress;
    use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
    use serde::Deserialize;

    const NAMESPACE: &str = "config";

    pub struct Config {
        pub paired_id_address: BLEAddress,
        pub setup_finished: bool,
    }

    impl Config {
        pub fn read(nvs: &EspNvs<NvsDefault>) -> anyhow::Result<Self> {
            todo!()
        }
    }

    impl Config {
        pub fn get_paired_id_address(nvs: &EspNvs<NvsDefault>) -> anyhow::Result<Option<BLEAddress>> {
            let mut buf = [0u8; 6];
            nvs.get_blob("paired_id_address", &mut buf).context("failed to get paired_id_address")?;
            Ok(Some(BLEAddress::from_le_bytes(buf, esp32_nimble::BLEAddressType::PublicID)))
        }

        pub fn set_paired_id_address(nvs: &mut EspNvs<NvsDefault>, address: BLEAddress) -> anyhow::Result<()> {
            nvs.set_blob("paired_id_address", &address.as_le_bytes()).context("failed to set paired_id_address")
        }
    }

    pub fn nvs() -> anyhow::Result<EspNvs<NvsDefault>> {
        log::info!("take the default esp nvs partition");
        let default_partition = EspNvsPartition::<NvsDefault>::take().context("failed to take the default esp nvs partition")?;

        log::info!("failed to create nvs instance");
        let nvs = EspNvs::new(default_partition, NAMESPACE, true).context("failed to create nvs instance")?;

        Ok(nvs)
    }
}
mod bluetooth {
    use std::sync::Arc;

    use anyhow::Context;
    use esp32_nimble::{utilities::{mutex::Mutex, BleUuid}, uuid128, BLEAdvertisementData, BLEDevice, NimbleProperties};
    use esp_idf_svc::nvs::{EspNvs, NvsDefault};

    use crate::config::Config;

    const SERVICE_UUID: BleUuid = uuid128!("15274059-8c2f-4a3f-8130-0c240179d72f");
    const SETUP_CHARACTERISTIC_UUID: BleUuid = uuid128!("3dc80940-699f-4666-8dc7-a150d328eb27");
    const DATA_CHARACTERISTIC_UUID: BleUuid = uuid128!("6b0d555e-d962-4219-89ae-d7c32efa7dcf");

    pub fn initialize(nvs: Arc<Mutex<EspNvs<NvsDefault>>>) -> anyhow::Result<()> {
        let device = BLEDevice::take();

        let advertising = device.get_advertising();
        advertising.lock()
            .set_data(BLEAdvertisementData::new()
            .name("Weight Aware Bag")
            .add_service_uuid(SERVICE_UUID))
            .context("failed to set ble advertising data")?;

        let server = device.get_server();
        let service = server.create_service(SERVICE_UUID);
        let setup_characteristic = service.lock()
            .create_characteristic(SETUP_CHARACTERISTIC_UUID, NimbleProperties::WRITE);
        let data_characteristic = service.lock()
            .create_characteristic(DATA_CHARACTERISTIC_UUID, NimbleProperties::NOTIFY | NimbleProperties::READ);

        server.on_connect(move |server, conn_desc| {
            let paired = Config::get_paired_id_address(&nvs.lock());
            match paired {
                Ok(Some(paired)) => if paired != conn_desc.id_address() {
                    if let Err(error) = server.disconnect(conn_desc.conn_handle()) {
                        log::error!("failed to disconnect unpaired device: {:?}", error);
                    }
                }
                Ok(None) => {
                    if let Err(error) = Config::set_paired_id_address(&mut nvs.lock(), conn_desc.id_address()) {
                        log::error!("failed to set paired id address: {:?}", error);
                    }
                }
                Err(error) => log::error!("failed to get paired id address: {:?}", error),
            }

        });

        Ok(())
    }
}

use anyhow::Context;
use esp32_nimble::utilities::mutex::Mutex;
use std::sync::Arc;
use esp_idf_svc::{hal::prelude::Peripherals, nvs::{EspNvs, EspNvsPartition, NvsDefault}};

fn main() -> anyhow::Result<()> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("initializing nvs flash storage");
    let nvs = config::nvs().context("failed to initialize nvs flash storage")?;
    let nvs = Arc::new(Mutex::new(nvs));

    Ok(())
}
