#![no_std]
#![no_main]

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
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    main,
    spi::master::{Config as SpiConfig, Spi},
    time::Rate,
};
use heapless::String;
use log::info;
use mipidsi::{
    interface::SpiInterface,
    models::ST7789,
    options::{ColorInversion, Orientation, Rotation},
};

esp_bootloader_esp_idf::esp_app_desc!();

/// LILYGO TTGO T-Display 1.14" ST7789（橫向 240×135）
const LCD_W: i32 = 240;
const LCD_H: i32 = 135;
const VISIBLE_LINES: usize = 9;
const LINE_CAP: usize = 40;

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

fn draw_header<D>(display: &mut D)
where
    D: DrawTarget<Color = Rgb565>,
{
    let _ = Rectangle::new(Point::zero(), Size::new(LCD_W as u32, 22))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLUE))
        .draw(display);

    let style = MonoTextStyle::new(&FONT_9X15_BOLD, Rgb565::YELLOW);
    let _ = Text::new("T-Display LOG", Point::new(8, 16), style).draw(display);
}

#[main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    let mut delay = Delay::new();
    let output_cfg = OutputConfig::default();

    // T-Display: MOSI=19, SCLK=18, CS=5, DC=16, RST=23, BL=4
    let _backlight = Output::new(peripherals.GPIO4, Level::High, output_cfg);

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
    draw_header(&mut display);

    let mut lcd_log = LcdLog::new();
    lcd_log.push("LCD ready");
    lcd_log.push("USB monitor still works");
    lcd_log.draw(&mut display);

    info!("LCD 已啟動，log 會同時出現在螢幕與 USB monitor");

    loop {
        lcd_log.push("LED ON");
        lcd_log.draw(&mut display);
        info!("LED 亮起");
        delay.delay_millis(1000);

        lcd_log.push("LED OFF");
        lcd_log.draw(&mut display);
        info!("LED 熄滅");
        delay.delay_millis(1000);
    }
}
