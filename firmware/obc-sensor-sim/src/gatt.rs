use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use heapless::Vec;
use obc_sensor_sim::{control::POWER_FEATURES, Sensor};
use trouble_host::prelude::*;

pub type Server<'a> = AttributeServer<'a, NoopRawMutex, DefaultPacketPool, 40, 1>;
pub type Measurement = Characteristic<Vec<u8, 8>>;
pub type ControlPoint = Characteristic<Vec<u8, 20>>;

pub fn server(sensor: Sensor, control_store: &mut [u8; 20]) -> (Server<'_>, Measurement, Option<ControlPoint>) {
    let mut table = AttributeTable::new();
    {
        let mut gap = table.add_service(Service::new(0x1800u16));
        gap.add_characteristic_ro(0x2a00u16, sensor.name()).build();
        let appearance = match sensor {
            Sensor::Power => &1156u16,
            Sensor::Cadence => &1155u16,
            Sensor::HeartRate => &833u16,
        };
        gap.add_characteristic_ro(0x2a01u16, appearance).build();
    }
    table.add_service(Service::new(0x1801u16));
    {
        let mut dis = table.add_service(Service::new(0x180au16));
        dis.add_characteristic_ro(0x2a29u16, "OpenBikeComputer").build();
        dis.add_characteristic_ro(0x2a24u16, "nRF54L15 sensor simulator").build();
        dis.add_characteristic_ro(0x2a26u16, env!("CARGO_PKG_VERSION")).build();
    }
    table.add_service(Service::new(0x180fu16)).add_characteristic_ro(0x2a19u16, &90u8).build();
    let (measurement, control) = {
        let mut service = table.add_service(Service::new(sensor.service()));
        let measurement = service
            .add_characteristic_small(sensor.measurement(), [CharacteristicProp::Notify], Vec::<u8, 8>::new())
            .build();
        match sensor {
            Sensor::Power => {
                service.add_characteristic_ro(0x2a65u16, &POWER_FEATURES).build();
                service.add_characteristic_ro(0x2a5du16, &5u8).build();
                let control = service
                    .add_characteristic(
                        0x2a66u16,
                        [CharacteristicProp::Write, CharacteristicProp::Indicate],
                        Vec::<u8, 20>::new(),
                        control_store,
                    )
                    .build();
                (measurement, Some(control))
            }
            Sensor::Cadence => {
                service.add_characteristic_ro(0x2a5cu16, &2u16).build();
                service.add_characteristic_ro(0x2a5du16, &5u8).build();
                (measurement, None)
            }
            Sensor::HeartRate => {
                service.add_characteristic_ro(0x2a38u16, &1u8).build();
                (measurement, None)
            }
        }
    };
    (AttributeServer::new(table), measurement, control)
}
