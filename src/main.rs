#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::Write;

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::{Config as NetConfig, Runner, StackResources};
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    mono_font::{
        ascii::{FONT_6X10, FONT_9X15_BOLD},
        MonoTextStyle,
    },
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    ram,
    rng::Rng,
    spi::master::{Config as SpiConfig, Spi},
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_radio::wifi::{
    AuthenticationMethodConfig, Config as WifiConfig, ControllerConfig, Interface, WifiController,
    scan::ScanConfig, sta::StationConfig,
};
use heapless::String;
use log::info;
use mipidsi::{
    interface::SpiInterface,
    models::ST7789,
    options::{ColorInversion, Orientation, Rotation},
};

mod buttons;
mod weather;

esp_bootloader_esp_idf::esp_app_desc!();

/// 來自 `wifi.env` 或環境變數 `WIFI_SSID` / `WIFI_PASSWORD`（由 build.rs 寫入 OUT_DIR）。
const WIFI_SSID: &str = include_str!(concat!(env!("OUT_DIR"), "/wifi_ssid.txt"));
const WIFI_PASSWORD: &str = include_str!(concat!(env!("OUT_DIR"), "/wifi_password.txt"));
const WEATHER_CITY: &str = include_str!(concat!(env!("OUT_DIR"), "/weather_city.txt"));
const WEATHER_LAT: &str = include_str!(concat!(env!("OUT_DIR"), "/weather_lat.txt"));
const WEATHER_LON: &str = include_str!(concat!(env!("OUT_DIR"), "/weather_lon.txt"));

fn wifi_credentials_set() -> bool {
    !matches!(WIFI_SSID, "" | "YOUR_SSID")
}

/// LILYGO TTGO T-Display 1.14" ST7789（橫向 240×135）
const LCD_W: i32 = 240;
const LCD_H: i32 = 135;
const VISIBLE_LINES: usize = 9;
const LINE_CAP: usize = 40;

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        STATIC_CELL.uninit().write(($val))
    }};
}

struct LcdLog {
    lines: [String<LINE_CAP>; VISIBLE_LINES],
    next: usize,
    filled: usize,
}

impl LcdLog {
    fn new() -> Self {
        Self {
            lines: core::array::from_fn(|_| String::new()),
            next: 0,
            filled: 0,
        }
    }

    fn push(&mut self, text: &str) {
        self.lines[self.next].clear();
        let _ = self.lines[self.next].push_str(truncate_ascii(text));
        self.next = (self.next + 1) % VISIBLE_LINES;
        if self.filled < VISIBLE_LINES {
            self.filled += 1;
        }
    }

    fn push_fmt(&mut self, args: core::fmt::Arguments<'_>) {
        let mut line: String<LINE_CAP> = String::new();
        let _ = line.write_fmt(args);
        self.push(&line);
    }

    fn draw<D>(&self, display: &mut D)
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let _ = Rectangle::new(Point::new(0, 22), Size::new(LCD_W as u32, (LCD_H - 22) as u32))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(display);

        let style = MonoTextStyle::new(&FONT_6X10, Rgb565::GREEN);
        let start = if self.filled < VISIBLE_LINES {
            0
        } else {
            self.next
        };

        for i in 0..self.filled {
            let idx = (start + i) % VISIBLE_LINES;
            let y = 32 + (i as i32) * 11;
            let _ = Text::new(self.lines[idx].as_str(), Point::new(6, y), style).draw(display);
        }
    }
}

fn truncate_ascii(text: &str) -> &str {
    let max_chars = (LCD_W as usize) / 6;
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => &text[..idx],
        None => text,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Weather,
    Wifi,
}

impl Page {
    fn toggle(self) -> Self {
        match self {
            Self::Weather => Self::Wifi,
            Self::Wifi => Self::Weather,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Weather => "Weather",
            Self::Wifi => "Wi-Fi",
        }
    }
}

fn draw_header<D>(display: &mut D, title: &str)
where
    D: DrawTarget<Color = Rgb565>,
{
    let _ = Rectangle::new(Point::zero(), Size::new(LCD_W as u32, 22))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLUE))
        .draw(display);

    let style = MonoTextStyle::new(&FONT_9X15_BOLD, Rgb565::YELLOW);
    let _ = Text::new(title, Point::new(8, 16), style).draw(display);
}

fn wifi_auth() -> AuthenticationMethodConfig {
    if WIFI_PASSWORD.is_empty() {
        AuthenticationMethodConfig::Open
    } else {
        AuthenticationMethodConfig::Wpa2Personal(WIFI_PASSWORD.try_into().unwrap())
    }
}

#[esp_hal::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    let mut delay = Delay::new();
    let output_cfg = OutputConfig::default();

    // Wi-Fi 韌體需要 heap。ESP32 還要把一部分放回被回收的 DRAM。
    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    // T-Display: MOSI=19, SCLK=18, CS=5, DC=16, RST=23, BL=4
    // 左鍵 GPIO0（BOOT）、右鍵 GPIO35（僅輸入，板上已上拉）
    let _backlight = Output::new(peripherals.GPIO4, Level::High, output_cfg);
    let btn_left = Input::new(
        peripherals.GPIO0,
        InputConfig::default().with_pull(Pull::Up),
    );
    let btn_right = Input::new(peripherals.GPIO35, InputConfig::default());

    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(26))
            .with_mode(esp_hal::spi::Mode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO18)
    .with_mosi(peripherals.GPIO19);

    let cs = Output::new(peripherals.GPIO5, Level::High, output_cfg);
    let dc = Output::new(peripherals.GPIO16, Level::Low, output_cfg);
    let rst = Output::new(peripherals.GPIO23, Level::High, output_cfg);
    let spi_device = ExclusiveDevice::new_no_delay(spi, cs).unwrap();

    let mut buffer = [0u8; 512];
    let di = SpiInterface::new(spi_device, dc, &mut buffer);

    let mut display = mipidsi::Builder::new(ST7789, di)
        .display_size(135, 240)
        .display_offset(52, 40)
        .invert_colors(ColorInversion::Inverted)
        .orientation(Orientation::new().rotate(Rotation::Deg90))
        .reset_pin(rst)
        .init(&mut delay)
        .unwrap();

    let _ = display.clear(Rgb565::BLACK);
    draw_header(&mut display, "T-Display WiFi");

    let mut lcd_log = LcdLog::new();
    lcd_log.push("LCD ready");
    lcd_log.push("starting radio...");
    lcd_log.draw(&mut display);

    // Wi-Fi 一定要先開 scheduler，才能 init radio。
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);
    spawner.spawn(buttons::watch(btn_left, buttons::Event::TogglePage).unwrap());
    spawner.spawn(buttons::watch(btn_right, buttons::Event::Refresh).unwrap());

    let station_config = if wifi_credentials_set() {
        WifiConfig::Station(
            StationConfig::default()
                .with_ssid(WIFI_SSID.try_into().unwrap())
                .with_authentication(wifi_auth()),
        )
    } else {
        WifiConfig::Station(StationConfig::default())
    };

    lcd_log.push("wifi init");
    lcd_log.draw(&mut display);

    let wifi_interface = Interface::station();
    let mut controller = WifiController::new(
        peripherals.WIFI,
        ControllerConfig::default().with_initial_config(station_config),
    )
    .expect("wifi init failed");

    lcd_log.push("scanning...");
    lcd_log.draw(&mut display);
    info!("開始掃描 Wi-Fi");

    let scan_config = ScanConfig::default().with_max(12);
    match controller.scan_async(&scan_config).await {
        Ok(mut aps) => {
            aps.sort_by(|a, b| b.signal_strength.cmp(&a.signal_strength));
            info!("掃到 {} 個 AP", aps.len());
            lcd_log.push_fmt(format_args!("found {} AP", aps.len()));

            let mut target_seen = false;
            for ap in aps.iter().take(VISIBLE_LINES.saturating_sub(2)) {
                let name = if ap.ssid.is_empty() {
                    "<hidden>"
                } else {
                    ap.ssid.as_str()
                };
                if name == WIFI_SSID {
                    target_seen = true;
                }
                lcd_log.push_fmt(format_args!("{:>3} {}", ap.signal_strength, name));
                info!(
                    "AP ssid={:?} rssi={} ch={}",
                    name, ap.signal_strength, ap.channel
                );
            }

            if !wifi_credentials_set() {
                lcd_log.push("edit wifi.env to connect");
            } else if target_seen {
                lcd_log.push_fmt(format_args!("saw {WIFI_SSID}"));
            } else {
                lcd_log.push_fmt(format_args!("no {WIFI_SSID}"));
            }
        }
        Err(err) => {
            info!("掃描失敗: {err:?}");
            lcd_log.push("scan failed");
        }
    }
    lcd_log.draw(&mut display);

    let net_config = NetConfig::dhcpv4(Default::default());
    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        net_config,
        mk_static!(StackResources<5>, StackResources::<5>::new()),
        seed,
    );

    spawner.spawn(net_task(runner).unwrap());

    if !wifi_credentials_set() {
        lcd_log.push("scan-only mode");
        lcd_log.draw(&mut display);
        info!("尚未設定 SSID，只掃描不連線。請改 wifi.env 或設 WIFI_SSID");
        loop {
            Timer::after(Duration::from_secs(5)).await;
        }
    }

    spawner.spawn(connection_task(controller).unwrap());

    lcd_log.push("connecting...");
    lcd_log.draw(&mut display);
    info!("等待連上 {WIFI_SSID} 並取得 DHCP");

    loop {
        if let Some(v4) = stack.config_v4() {
            lcd_log.push("connected");
            lcd_log.push_fmt(format_args!("IP {}", v4.address));
            lcd_log.draw(&mut display);
            info!("已連上，IP = {}", v4.address);
            break;
        }
        Timer::after(Duration::from_secs(1)).await;
    }

    let tcp_state = mk_static!(weather::WeatherTcpState, weather::WeatherTcpState::new());
    let mut weather_url: String<192> = String::new();
    let _ = write!(
        weather_url,
        "http://api.open-meteo.com/v1/forecast?latitude={WEATHER_LAT}&longitude={WEATHER_LON}&current=temperature_2m,weather_code"
    );

    let mut page = Page::Weather;
    let mut last_weather: Option<weather::Weather> = None;
    let mut last_error: Option<String<48>> = None;
    let mut fetch_now = true;

    loop {
        let online = stack.config_v4();
        let mut ip_line: String<LINE_CAP> = String::new();
        let wifi_status = match &online {
            Some(v4) => {
                let _ = write!(ip_line, "IP {}", v4.address);
                "online"
            }
            None => {
                let _ = ip_line.push_str("IP --");
                "offline"
            }
        };

        if fetch_now {
            match &online {
                None => {
                    info!("Wi-Fi / DHCP 斷線，等待重連");
                    fetch_now = false;
                }
                Some(_) => {
                    if page == Page::Weather {
                        draw_header(&mut display, page.title());
                        weather::draw_status(&mut display, "weather", "fetching...");
                    }
                    info!("抓取天氣 {WEATHER_CITY} {weather_url}");
                    match select(
                        weather::fetch(stack, tcp_state, &weather_url, WEATHER_CITY),
                        buttons::EVENTS.receive(),
                    )
                    .await
                    {
                        Either::First(Ok(wx)) => {
                            info!("{} {} {}", wx.place, wx.temp_c, wx.condition);
                            last_weather = Some(wx);
                            last_error = None;
                            fetch_now = false;
                        }
                        Either::First(Err(err)) => {
                            info!("抓天氣失敗: {err}");
                            last_error = Some(err);
                            fetch_now = false;
                        }
                        Either::Second(buttons::Event::TogglePage) => {
                            page = page.toggle();
                            fetch_now = false;
                            info!("抓天氣取消，切到 {}", page.title());
                        }
                        Either::Second(buttons::Event::Refresh) => {
                            info!("抓取中又按刷新，重試");
                        }
                    }
                }
            }
            for _ in 0..buttons::drain_toggles() {
                page = page.toggle();
            }
        }

        draw_header(&mut display, page.title());
        match page {
            Page::Weather => match last_weather.as_ref() {
                Some(wx) => weather::draw(&mut display, wx, &ip_line),
                None => {
                    if let Some(err) = last_error.as_ref() {
                        weather::draw_status(&mut display, "wx failed", err);
                    } else if online.is_none() {
                        weather::draw_status(&mut display, "offline", "waiting wifi");
                    } else {
                        weather::draw_status(&mut display, "weather", "no data");
                    }
                }
            },
            Page::Wifi => {
                let ssid = if wifi_credentials_set() {
                    WIFI_SSID
                } else {
                    "(unset)"
                };
                weather::draw_wifi(&mut display, ssid, &ip_line, wifi_status);
            }
        }

        let idle = if online.is_some() {
            Duration::from_secs(600)
        } else {
            Duration::from_secs(5)
        };
        match buttons::wait(idle).await {
            Some(buttons::Event::Refresh) => {
                info!("右鍵：刷新天氣");
                page = Page::Weather;
                fetch_now = true;
            }
            Some(buttons::Event::TogglePage) => {
                page = page.toggle();
                info!("左鍵：切到 {}", page.title());
            }
            None => {
                info!("定期刷新天氣");
                fetch_now = true;
            }
        }
    }
}

#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) {
    loop {
        info!("嘗試連線到 {WIFI_SSID}");
        match controller.connect_async().await {
            Ok(info) => {
                info!("Wi-Fi 已關聯: {info:?}");
                let _ = controller.wait_for_disconnect_async().await;
                info!("Wi-Fi 已斷線，5 秒後重連");
            }
            Err(err) => {
                info!("連線失敗: {err:?}");
            }
        }
        Timer::after(Duration::from_secs(5)).await;
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}
