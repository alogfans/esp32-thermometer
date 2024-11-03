#![no_std]
#![no_main]

use dht20::Dht20;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    mono_font::{ascii::FONT_10X20, MonoTextStyleBuilder},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyleBuilder, Rectangle, StyledDrawable},
    text::{Baseline, Text},
};
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::{
    delay::Delay,
    gpio::Io,
    i2c::I2c,
    peripherals::{Peripherals, I2C0, I2C1},
    prelude::*,
    rng::Rng,
    timer::timg::TimerGroup,
    Blocking,
};
use esp_println::println;
use ssd1306::{
    mode::{BufferedGraphicsMode, DisplayConfig},
    prelude::*,
    size::DisplaySize128x64,
    I2CDisplayInterface, Ssd1306,
};

use esp_wifi::{
    init,
    wifi::{
        ClientConfiguration, Configuration, WifiController, WifiDevice, WifiEvent, WifiStaDevice,
        WifiState,
    },
    EspWifiInitFor,
};

use core::cell::RefCell;
use embassy_net::{tcp::TcpSocket, Ipv4Address, Stack, StackResources};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

struct Record {
    pub temp: f32,
    pub hum: f32,
}

impl Record {
    fn new() -> Self {
        Record {
            temp: 0.0,
            hum: 0.0,
        }
    }
}

static SHARED_RECORD: Mutex<CriticalSectionRawMutex, RefCell<Record>> =
    Mutex::new(RefCell::new(Record {
        temp: 0.0,
        hum: 0.0,
    }));

fn setup_devices(
    peripherals: Peripherals,
) -> (
    Dht20<I2c<'static, I2C0, Blocking>, esp_hal::delay::Delay>,
    Ssd1306<
        I2CInterface<I2c<'static, I2C1, Blocking>>,
        DisplaySize128x64,
        BufferedGraphicsMode<DisplaySize128x64>,
    >,
    &'static Stack<WifiDevice<'static, WifiStaDevice>>,
    WifiController<'static>,
) {
    let io = Io::new(peripherals.GPIO, peripherals.IO_MUX);
    let i2c0 = I2c::new(
        peripherals.I2C0,
        io.pins.gpio32, /*sda*/
        io.pins.gpio33, /*scl*/
        100.kHz(),
    );
    let i2c1 = I2c::new(
        peripherals.I2C1,
        io.pins.gpio21, /*sda*/
        io.pins.gpio22, /*scl*/
        100.kHz(),
    );

    let sensor = Dht20::new(i2c0, 0x38, Delay::new());
    let display_device = I2CDisplayInterface::new(i2c1);
    let mut display = Ssd1306::new(display_device, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().unwrap();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let init = init(
        EspWifiInitFor::Wifi,
        timg0.timer0,
        Rng::new(peripherals.RNG),
        peripherals.RADIO_CLK,
    )
    .unwrap();

    let wifi = peripherals.WIFI;
    let (wifi_interface, controller) =
        esp_wifi::wifi::new_with_mode(&init, wifi, WifiStaDevice).unwrap();

    let timg1 = TimerGroup::new(peripherals.TIMG1);
    esp_hal_embassy::init(timg1.timer0);

    let config = embassy_net::Config::dhcpv4(Default::default());
    let seed = 1234;
    let stack = &*mk_static!(
        Stack<WifiDevice<'_, WifiStaDevice>>,
        Stack::new(
            wifi_interface,
            config,
            mk_static!(StackResources<3>, StackResources::<3>::new()),
            seed
        )
    );

    (sensor, display, stack, controller)
}

#[embassy_executor::task]
async fn display_loop(
    display: Ssd1306<
        I2CInterface<I2c<'static, I2C1, Blocking>>,
        DisplaySize128x64,
        BufferedGraphicsMode<DisplaySize128x64>,
    >,
) {
    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(BinaryColor::On)
        .build();

    let line_style = PrimitiveStyleBuilder::new()
        .stroke_width(1)
        .stroke_color(BinaryColor::On)
        .build();

    let mut display_buf = [0u8; 64];
    let mut display = display;

    loop {
        let mut record = Record::new();
        SHARED_RECORD.lock(|f| {
            record.hum = f.borrow().hum;
            record.temp = f.borrow().temp;
        });

        let display_content = format_no_std::show(
            &mut display_buf,
            format_args!(" TEMP  HUM \n {:02.2} {:02.2}\n", record.temp, record.hum),
        )
        .unwrap();

        display.clear(BinaryColor::Off).unwrap();

        Rectangle::new(Point::new(1, 1), Size::new(128 - 2, 64 - 2))
            .draw_styled(&line_style, &mut display)
            .unwrap();

        Line::new(Point { x: 64, y: 1 }, Point { x: 64, y: 64 - 2 })
            .draw_styled(&line_style, &mut display)
            .unwrap();

        Text::with_baseline(
            display_content,
            Point::new(1, 10),
            text_style,
            Baseline::Top,
        )
        .draw(&mut display)
        .unwrap();

        display.flush().unwrap();
        Timer::after(Duration::from_millis(2_000)).await;
    }
}

#[embassy_executor::task]
async fn collect_sensor_loop(sensor: Dht20<I2c<'static, I2C0, Blocking>, esp_hal::delay::Delay>) {
    let mut sensor = sensor;
    loop {
        match sensor.read() {
            Ok(reading) => {
                SHARED_RECORD.lock(|f| {
                    f.borrow_mut().hum = reading.hum;
                    f.borrow_mut().temp = reading.temp;
                });
                esp_println::println!(
                    "Temperature: {:.2}, Humidity: {:.2}",
                    reading.temp,
                    reading.hum
                );
            }
            Err(e) => {
                log::error!("Error reading sensor: {e:?}");
            }
        }
        Timer::after(Duration::from_millis(2_000)).await;
    }
}

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");

#[embassy_executor::task]
async fn wifi_establish_conection(mut controller: WifiController<'static>) {
    println!("start connection task");
    println!("Device capabilities: {:?}", controller.get_capabilities());
    loop {
        match esp_wifi::wifi::get_wifi_state() {
            WifiState::StaConnected => {
                // wait until we're no longer connected
                controller.wait_for_event(WifiEvent::StaDisconnected).await;
                Timer::after(Duration::from_millis(5000)).await
            }
            _ => {}
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = Configuration::Client(ClientConfiguration {
                ssid: SSID.try_into().unwrap(),
                password: PASSWORD.try_into().unwrap(),
                ..Default::default()
            });
            controller.set_configuration(&client_config).unwrap();
            println!("Starting wifi");
            controller.start().await.unwrap();
            println!("Wifi started!");
        }
        println!("About to connect...");

        match controller.connect().await {
            Ok(_) => println!("Wifi connected!"),
            Err(e) => {
                println!("Failed to connect to wifi: {e:?}");
                Timer::after(Duration::from_millis(5000)).await
            }
        }
    }
}

#[embassy_executor::task]
async fn wifi_stack_loop(stack: &'static Stack<WifiDevice<'static, WifiStaDevice>>) {
    stack.run().await
}

#[embassy_executor::task]
async fn wifi_ping_loop(stack: &'static Stack<WifiDevice<'static, WifiStaDevice>>) {
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    println!("Waiting to get IP address...");
    loop {
        if let Some(config) = stack.config_v4() {
            println!("Got IP: {}", config.address);
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    loop {
        Timer::after(Duration::from_millis(1_000)).await;
        let mut socket = TcpSocket::new(&stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(embassy_time::Duration::from_secs(10)));
        let remote_endpoint = (Ipv4Address::new(192, 168, 0, 110), 5000);
        println!("connecting...");
        let r = socket.connect(remote_endpoint).await;
        if let Err(e) = r {
            println!("connect error: {:?}", e);
            continue;
        }
        println!("connected!");
        let mut buf = [0; 1024];

        let mut request_buf = [0u8; 256];

        loop {
            let mut record = Record::new();
            SHARED_RECORD.lock(|f| {
                record.hum = f.borrow().hum;
                record.temp = f.borrow().temp;
            });

            use embedded_io_async::Write;
            let request = format_no_std::show(
                &mut request_buf,
                format_args!("GET /append?temperature={}&humidity={} HTTP/1.0\r\nHost: 192.168.0.110\r\n\r\n", record.temp, record.hum),
            )
            .unwrap();
            let r = socket.write_all(request.as_bytes()).await;
            if let Err(e) = r {
                println!("write error: {:?}", e);
                break;
            }
            let n = match socket.read(&mut buf).await {
                Ok(0) => {
                    println!("read EOF");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    println!("read error: {:?}", e);
                    break;
                }
            };
            println!("{}", core::str::from_utf8(&buf[..n]).unwrap());
        }
        Timer::after(Duration::from_millis(60000)).await;
    }
}

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    #[allow(unused)]
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(72 * 1024);
    let (sensor, display, stack, controller) = setup_devices(peripherals);

    spawner.spawn(wifi_establish_conection(controller)).ok();
    spawner.spawn(wifi_stack_loop(&stack)).ok();
    spawner.spawn(wifi_ping_loop(&stack)).ok();
    spawner.spawn(collect_sensor_loop(sensor)).ok();
    spawner.spawn(display_loop(display)).ok();
}
