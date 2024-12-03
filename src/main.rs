mod config {
    use anyhow::Context;
    use esp32_nimble::BLEAddress;
    use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};

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
        pub fn read_paired_id_address(
            nvs: &EspNvs<NvsDefault>,
        ) -> anyhow::Result<Option<BLEAddress>> {
            let mut buf = [0u8; 6];
            nvs.get_blob("paired_id_address", &mut buf)
                .context("failed to get paired_id_address")?;
            Ok(Some(BLEAddress::from_le_bytes(
                buf,
                esp32_nimble::BLEAddressType::PublicID,
            )))
        }

        pub fn write_paired_id_address(
            nvs: &mut EspNvs<NvsDefault>,
            address: BLEAddress,
        ) -> anyhow::Result<()> {
            nvs.set_blob("paired_id_address", &address.as_le_bytes())
                .context("failed to set paired_id_address")
        }
    }

    impl Config {
        pub fn read_setup_finished(nvs: &EspNvs<NvsDefault>) -> anyhow::Result<Option<bool>> {
            nvs.get_u8("setup_finished")
                .map(|value| value.map(|v| v != 0))
                .context("failed to get setup_finished")
        }

        pub fn write_setup_finished(
            nvs: &mut EspNvs<NvsDefault>,
            value: bool,
        ) -> anyhow::Result<()> {
            nvs.set_u8("setup_finished", value as u8)
                .context("failed to set setup_finished")
        }
    }

    pub fn nvs() -> anyhow::Result<EspNvs<NvsDefault>> {
        log::info!("take the default esp nvs partition");
        let default_partition = EspNvsPartition::<NvsDefault>::take()
            .context("failed to take the default esp nvs partition")?;

        log::info!("failed to create nvs instance");
        let nvs = EspNvs::new(default_partition, NAMESPACE, true)
            .context("failed to create nvs instance")?;

        Ok(nvs)
    }
}
mod bluetooth {
    use std::sync::Arc;

    use anyhow::Context;
    use esp32_nimble::{
        utilities::{mutex::Mutex, BleUuid},
        uuid128, BLEAdvertisementData, BLECharacteristic, BLEDevice, NimbleProperties,
    };
    use esp_idf_svc::nvs::{EspNvs, NvsDefault};

    use crate::config::Config;

    const SERVICE_UUID: BleUuid = uuid128!("15274059-8c2f-4a3f-8130-0c240179d72f");
    const SETUP_CHARACTERISTIC_UUID: BleUuid = uuid128!("3dc80940-699f-4666-8dc7-a150d328eb27");
    const DATA_CHARACTERISTIC_UUID: BleUuid = uuid128!("6b0d555e-d962-4219-89ae-d7c32efa7dcf");

    pub struct BluetoothCharacteristics {
        pub setup: Arc<Mutex<BLECharacteristic>>,
        pub data: Arc<Mutex<BLECharacteristic>>,
    }

    pub fn initialize(
        nvs: Arc<Mutex<EspNvs<NvsDefault>>>,
    ) -> anyhow::Result<BluetoothCharacteristics> {
        let device = BLEDevice::take();
        let server = device.get_server();
        let service = server.create_service(SERVICE_UUID);
        let setup_characteristic = service
            .lock()
            .create_characteristic(SETUP_CHARACTERISTIC_UUID, NimbleProperties::WRITE);
        let data_characteristic = service.lock().create_characteristic(
            DATA_CHARACTERISTIC_UUID,
            NimbleProperties::NOTIFY | NimbleProperties::READ,
        );

        server.on_connect(move |server, desc| {
            log::info!("new connection: {desc:?}");

            let paired = Config::read_paired_id_address(&nvs.lock());
            match paired {
                Ok(Some(paired)) => {
                    if paired != desc.id_address() {
                        log::info!("connection is unrecognized, disconnecting");

                        if let Err(error) = server.disconnect(desc.conn_handle()) {
                            log::error!("failed to disconnect unpaired device: {error:?}");
                            return;
                        }
                    }
                }
                Ok(None) => {
                    if let Err(error) =
                        Config::write_paired_id_address(&mut nvs.lock(), desc.id_address())
                    {
                        log::error!("failed to set paired id address: {error:?}");
                    }

                    log::info!("paired with device: {desc:?}");
                }
                Err(error) => {
                    log::error!("failed to get paired id address: {error:?}");
                    return;
                }
            }

            if let Err(error) = server.update_conn_params(desc.conn_handle(), 24, 48, 0, 60) {
                log::error!("failed to update connection parameters: {error:?}");
            }
        });

        server.on_disconnect(move |desc, reason| {
            log::info!("client {desc:?} disconnected with reason {reason:?}");
        });

        log::info!("starting ble advertising");
        let advertising = device.get_advertising();
        advertising
            .lock()
            .set_data(
                BLEAdvertisementData::new()
                    .name("Weight Aware Bag")
                    .add_service_uuid(SERVICE_UUID),
            )
            .context("failed to set ble advertising data")?;

        server.ble_gatts_show_local();

        Ok(BluetoothCharacteristics {
            setup: setup_characteristic,
            data: data_characteristic,
        })
    }
}
mod movement {
    use anyhow::{anyhow, Context};
    use esp_idf_svc::hal::{
        delay::Delay,
        gpio::AnyIOPin,
        i2c::{I2c, I2cConfig, I2cDriver},
        peripheral::Peripheral,
        units::{Hertz, KiloHertz},
    };
    use mpu6050::Mpu6050;

    pub fn initialize<'d>(
        i2c: impl Peripheral<P = impl I2c> + 'd,
        sda: AnyIOPin,
        scl: AnyIOPin,
        baudrate: Hertz,
    ) -> anyhow::Result<()> {
        let config = I2cConfig::new().baudrate(baudrate);

        log::info!("initializing i2c driver");
        let driver =
            I2cDriver::new(i2c, sda, scl, &config).context("failed to initialize i2c driver")?;

        log::info!("initializing mpu6050");
        let mut mpu6050 = Mpu6050::new(driver);

        let mut delay = Delay::new_default();
        mpu6050
            .init(&mut delay)
            .map_err(|e| anyhow!("failed to initialize mpu6050: {e:?}"))?;

        Ok(())
    }
}

use anyhow::Context;
use esp32_nimble::utilities::mutex::Mutex;
use esp_idf_svc::hal::{
    prelude::Peripherals,
    units::{Hertz, KiloHertz},
};
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("initializing nvs flash storage");
    let nvs = config::nvs().context("failed to initialize nvs flash storage")?;
    let nvs = Arc::new(Mutex::new(nvs));

    log::info!("initializing bluetooth");
    let characteristics = bluetooth::initialize(nvs).context("failed to initialize bluetooth")?;

    log::info!("initializing peripherals");
    let peripherals = Peripherals::take().context("failed to take peripherals")?;

    log::info!("initializing movement sensor");
    let i2c = movement::initialize(
        peripherals.i2c0,
        peripherals.pins.gpio21.into(),
        peripherals.pins.gpio22.into(),
        KiloHertz(100).into(),
    )
    .context("failed to initialize movement sensor")?;

    Ok(())
}
