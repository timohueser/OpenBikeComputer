#![no_std]
#![no_main]

mod board;
mod gatt;

use core::cell::{Cell, RefCell};
use defmt::{info, unwrap, warn};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Ticker, Timer};
use obc_sensor_sim::{
    control::{Control, Response},
    input::{Button, Press},
    Simulator,
};
use panic_probe as _;
use trouble_host::prelude::*;

type StackType<'a> = Stack<'a, nrf_sdc::SoftdeviceController<'static>, DefaultPacketPool>;

fn now() -> u64 {
    Instant::now().as_millis()
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let (controller, controls, factory) = board::init(spawner);
    let mut resources = HostResources::<_, DefaultPacketPool, 1, 2>::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random(obc_sensor_sim::Sensor::Power.address(factory)))
        .build();
    let sim = RefCell::new(Simulator::default());
    let changed = Signal::<NoopRawMutex, ()>::new();
    let connected = Cell::new(false);
    let mut runner = stack.runner();
    join3(
        async {
            unwrap!(runner.run().await);
        },
        buttons(controls, &sim, &changed, &connected),
        peripheral(&stack, factory, &sim, &changed, &connected),
    )
    .await;
}

async fn buttons(
    mut controls: board::Controls,
    sim: &RefCell<Simulator>,
    changed: &Signal<NoopRawMutex, ()>,
    connected: &Cell<bool>,
) {
    let mut buttons = [Button::default(); 4];
    let mut ticker = Ticker::every(Duration::from_millis(20));
    loop {
        ticker.next().await;
        let now = now();
        let mut sim = sim.borrow_mut();
        sim.tick(now);
        for (i, button) in buttons.iter_mut().enumerate() {
            let Some(press) = button.update(controls.buttons[i].is_low(), now, i >= 2) else { continue };
            match (i, press) {
                (0, Press::Short) => {
                    sim.next_sensor(now);
                    changed.signal(());
                }
                (0, Press::Long) => sim.stopped = !sim.stopped,
                (1, Press::Short) => sim.next_scenario(now),
                (1, Press::Long) => {
                    sim.online = !sim.online;
                    changed.signal(());
                }
                (2, Press::Step) => sim.adjust(false),
                (3, Press::Step) => sim.adjust(true),
                _ => {}
            }
            info!(
                "{}: base={} value={} scenario={} stopped={} online={}",
                sim.sensor.name(),
                sim.base(),
                sim.value(now),
                sim.scenario as u8,
                sim.stopped,
                sim.online
            );
        }
        let kind = sim.sensor as usize;
        let phase = now % 2400;
        // The selected sensor LED counts 1/2/3 pulses for the scenario; stopped is solid.
        let lit = sim.stopped || (phase < (sim.scenario as u64 + 1) * 400 && phase % 400 < 200);
        for i in 0..3 {
            controls.leds[i].set_level(if i == kind && lit {
                embassy_nrf::gpio::Level::High
            } else {
                embassy_nrf::gpio::Level::Low
            });
        }
        let link = sim.online && (connected.get() || now % 1000 < 500);
        controls.leds[3].set_level(if link { embassy_nrf::gpio::Level::High } else { embassy_nrf::gpio::Level::Low });
    }
}

async fn peripheral(
    stack: &StackType<'_>,
    factory: [u8; 6],
    sim: &RefCell<Simulator>,
    changed: &Signal<NoopRawMutex, ()>,
    connected: &Cell<bool>,
) {
    let mut peripheral = stack.peripheral();
    loop {
        changed.reset();
        let sensor = sim.borrow().sensor;
        if !sim.borrow().online {
            changed.wait().await;
            continue;
        }
        let mut store = [0; 20];
        let (server, measurement, control) = gatt::server(sensor, &mut store);
        let mut adv = [0; 31];
        let len = unwrap!(AdStructure::encode_slice(
            &[
                AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                AdStructure::CompleteServiceUuids16(&[sensor.service().to_le_bytes()]),
                AdStructure::CompleteLocalName(sensor.name().as_bytes()),
            ],
            &mut adv
        ));
        let sets = [AdvertisementSet {
            params: AdvertisementParameters {
                interval_min: Duration::from_millis(100),
                interval_max: Duration::from_millis(100),
                ..Default::default()
            },
            data: Advertisement::ConnectableScannableUndirected { adv_data: &adv[..len], scan_data: &[] },
            address: Some(Address::random(sensor.address(factory)).addr),
        }];
        let mut handles = AdvertisementSet::handles(&sets);
        let session = async {
            let advertiser = peripheral.advertise_ext(&sets, &mut handles).await?;
            info!("advertising {}", sensor.name());
            let connection = advertiser.accept().await?.with_attribute_server(&server)?;
            connected.set(true);
            info!("connected {}", sensor.name());
            serve(&connection, &server, measurement, control, sim).await;
            Ok::<(), BleHostError<nrf_sdc::Error>>(())
        };
        if let Either::First(Err(e)) = select(session, changed.wait()).await {
            warn!("BLE session: {:?}", defmt::Debug2Format(&e));
        }
        connected.set(false);
        // Let the runner complete a requested disconnect before starting a new advertiser.
        Timer::after_millis(100).await;
    }
}

async fn serve(
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    server: &gatt::Server<'_>,
    measurement: gatt::Measurement,
    control_point: Option<gatt::ControlPoint>,
    sim: &RefCell<Simulator>,
) {
    let control = RefCell::new(Control::default());
    let responses = Signal::<NoopRawMutex, Response>::new();
    let events = async {
        loop {
            match conn.next().await {
                GattConnectionEvent::Disconnected { .. } => return,
                GattConnectionEvent::Gatt { event } => {
                    let mut response = None;
                    let reply = match event {
                        GattEvent::Write(e) if control_point.as_ref().is_some_and(|cp| cp.handle == e.handle()) => {
                            let cp = control_point.as_ref().unwrap();
                            let mut cccd = [0; 2];
                            let subscribed = server.read(conn.raw(), cp.cccd_handle.unwrap(), 0, &mut cccd).is_ok()
                                && cccd == [2, 0];
                            let stopped = sim.borrow().rpm(now()) == 0;
                            let result = e.with_data(|offset, data| {
                                if offset != 0 {
                                    return Err(0x07);
                                }
                                control.borrow_mut().start(data, subscribed, stopped)
                            });
                            match result {
                                Ok(value) => {
                                    response = Some(value);
                                    e.accept()
                                }
                                Err(code) => e.reject(match code {
                                    0xfd => AttErrorCode::CCCD_IMPROPERLY_CONFIGURED,
                                    0xfe => AttErrorCode::PROCEDURE_ALREADY_IN_PROGRESS,
                                    0x07 => AttErrorCode::INVALID_OFFSET,
                                    _ => AttErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH,
                                }),
                            }
                        }
                        GattEvent::Write(e) => e.accept(),
                        GattEvent::Read(e) => e.accept(),
                        GattEvent::NotAllowed(e) => e.accept(),
                        GattEvent::Other(e) => e.accept(),
                    };
                    match reply {
                        Ok(reply) => {
                            reply.send().await;
                            if let Some(response) = response {
                                responses.signal(response);
                            }
                        }
                        Err(_) => return,
                    }
                }
                _ => {}
            }
        }
    };
    let notifications = async {
        let mut ticker = Ticker::every(Duration::from_secs(1));
        loop {
            ticker.next().await;
            let (bytes, len) = sim.borrow().measurement(now());
            if measurement.notify_raw(conn, &bytes[..len], false).await.is_err() {
                return;
            }
        }
    };
    let indications = async {
        loop {
            let mut response = responses.wait().await;
            if response.calibrating {
                Timer::after_secs(1).await;
                if sim.borrow().rpm(now()) != 0 {
                    response.bytes[2] = 4;
                    response.len = 3;
                }
            }
            let Some(cp) = control_point.as_ref() else { return };
            info!("power control: opcode={} result={}", response.bytes[1], response.bytes[2]);
            if cp.indicate_raw(conn, &response.bytes[..response.len], false).await.is_err() {
                return;
            }
            control.borrow_mut().confirmed();
        }
    };
    select(events, select(notifications, indications)).await;
}
