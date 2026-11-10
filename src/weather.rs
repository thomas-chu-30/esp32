use core::fmt::Write;

use embassy_net::{
    Stack,
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
};
use embassy_time::{Duration, with_timeout};
use embedded_graphics::{
    mono_font::{
        ascii::{FONT_6X10, FONT_10X20, FONT_9X15_BOLD},
        MonoTextStyle,
    },
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
};
use heapless::String;
use log::info;
use reqwless::{
    client::HttpClient,
    request::{Method, RequestBuilder},
};

use crate::{LCD_H, LCD_W};

const TCP_BUF: usize = 1500;
const FETCH_TIMEOUT: Duration = Duration::from_secs(12);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(8);
pub type WeatherTcpState = TcpClientState<1, TCP_BUF, TCP_BUF>;

pub struct Weather {
    pub temp_c: String<12>,
    pub condition: String<24>,
    pub place: String<24>,
}

pub async fn fetch(
    stack: Stack<'static>,
    tcp_state: &'static WeatherTcpState,
    url: &str,
    place: &str,
) -> Result<Weather, String<48>> {
    match with_timeout(FETCH_TIMEOUT, fetch_inner(stack, tcp_state, url, place)).await {
        Ok(result) => result,
        Err(_) => {
            info!("weather fetch timed out");
            Err(err_msg("timeout"))
        }
    }
}

async fn fetch_inner(
    stack: Stack<'static>,
    tcp_state: &'static WeatherTcpState,
    url: &str,
    place: &str,
) -> Result<Weather, String<48>> {
    let dns = DnsSocket::new(stack);
    let mut tcp = TcpClient::new(stack, tcp_state);
    tcp.set_timeout(Some(SOCKET_TIMEOUT));
    let mut client = HttpClient::new(&tcp, &dns);
    let mut rx_buf = [0u8; 2048];

    info!("weather dns/connect");
    let request = client
        .request(Method::GET, url)
        .await
        .map_err(http_err("dns/connect"))?;
    let mut request = request.headers(&[
        ("User-Agent", "esp32-tdisplay"),
        ("Accept", "application/json"),
        ("Connection", "close"),
    ]);
    info!("weather send");
    let response = request.send(&mut rx_buf).await.map_err(http_err("send"))?;
    let status = response.status;
    info!("weather body, status={status:?}");
    let body = response
        .body()
        .read_to_end()
        .await
        .map_err(http_err("body"))?;
    let text = core::str::from_utf8(body).map_err(|_| err_msg("utf8"))?;
    info!("weather HTTP {status:?}: {text}");

    parse_open_meteo(text, place)
}

pub fn draw<D>(display: &mut D, weather: &Weather, ip: &str)
where
    D: DrawTarget<Color = Rgb565>,
{
    let _ = Rectangle::new(Point::new(0, 22), Size::new(LCD_W as u32, (LCD_H - 22) as u32))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(display);

    let temp_style = MonoTextStyle::new(&FONT_10X20, Rgb565::YELLOW);
    let _ = Text::new(weather.temp_c.as_str(), Point::new(8, 52), temp_style).draw(display);

    let line = MonoTextStyle::new(&FONT_6X10, Rgb565::GREEN);
    let _ = Text::new(weather.condition.as_str(), Point::new(8, 74), line).draw(display);

    let place = MonoTextStyle::new(&FONT_6X10, Rgb565::CYAN);
    let _ = Text::new(weather.place.as_str(), Point::new(8, 90), place).draw(display);

    let meta = MonoTextStyle::new(&FONT_6X10, Rgb565::CSS_LIGHT_GRAY);
    let _ = Text::new(ip, Point::new(8, 108), meta).draw(display);
    let _ = Text::new("L page  R refresh", Point::new(8, 124), meta).draw(display);
}

pub fn draw_status<D>(display: &mut D, title: &str, detail: &str)
where
    D: DrawTarget<Color = Rgb565>,
{
    let _ = Rectangle::new(Point::new(0, 22), Size::new(LCD_W as u32, (LCD_H - 22) as u32))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(display);

    let title_style = MonoTextStyle::new(&FONT_9X15_BOLD, Rgb565::YELLOW);
    let _ = Text::new(title, Point::new(8, 48), title_style).draw(display);
    let detail_style = MonoTextStyle::new(&FONT_6X10, Rgb565::GREEN);
    let _ = Text::new(detail, Point::new(8, 70), detail_style).draw(display);
    let meta = MonoTextStyle::new(&FONT_6X10, Rgb565::CSS_LIGHT_GRAY);
    let _ = Text::new("L page  R refresh", Point::new(8, 124), meta).draw(display);
}

pub fn draw_wifi<D>(display: &mut D, ssid: &str, ip: &str, status: &str)
where
    D: DrawTarget<Color = Rgb565>,
{
    let _ = Rectangle::new(Point::new(0, 22), Size::new(LCD_W as u32, (LCD_H - 22) as u32))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(display);

    let label = MonoTextStyle::new(&FONT_6X10, Rgb565::CSS_LIGHT_GRAY);
    let value = MonoTextStyle::new(&FONT_9X15_BOLD, Rgb565::CYAN);

    let _ = Text::new("SSID", Point::new(8, 38), label).draw(display);
    let _ = Text::new(ssid, Point::new(8, 54), value).draw(display);
    let _ = Text::new("status", Point::new(8, 72), label).draw(display);
    let _ = Text::new(status, Point::new(8, 88), value).draw(display);
    let _ = Text::new(ip, Point::new(8, 108), label).draw(display);
    let _ = Text::new("L page  R refresh", Point::new(8, 124), label).draw(display);
}

fn parse_open_meteo(body: &str, place: &str) -> Result<Weather, String<48>> {
    // Open-Meteo 會先給 current_units（"°C"），真正的數字在 current 裡。
    let current = json_object(body, "current").ok_or_else(|| err_msg("no current"))?;
    let temp_raw = json_number(current, "temperature_2m").ok_or_else(|| err_msg("no temp"))?;
    let code_raw = json_number(current, "weather_code").ok_or_else(|| err_msg("no code"))?;
    let code: u8 = code_raw.parse().unwrap_or(255);

    let mut temp_c = String::new();
    let _ = write!(temp_c, "{temp_raw} C");

    let mut condition = String::new();
    let _ = condition.push_str(weather_text(code));

    let mut place_s = String::new();
    let _ = place_s.push_str(place.trim());

    Ok(Weather {
        temp_c,
        condition,
        place: place_s,
    })
}

fn json_object<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let mut pattern: String<24> = String::new();
    let _ = write!(pattern, "\"{key}\":{{");
    let start = body.find(pattern.as_str())?;
    let brace = start + pattern.len() - 1;
    let bytes = body.as_bytes();
    let mut depth = 0usize;
    for (offset, &byte) in bytes[brace..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&body[brace..=brace + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

fn json_number<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let mut pattern: String<32> = String::new();
    let _ = write!(pattern, "\"{key}\":");
    let rest = body.split(pattern.as_str()).nth(1)?.trim_start();
    if rest.starts_with('"') {
        return None;
    }
    let end = rest
        .find(|c: char| !matches!(c, '0'..='9' | '.' | '-' | '+'))
        .unwrap_or(rest.len());
    let value = rest[..end].trim();
    if value.is_empty() { None } else { Some(value) }
}

fn weather_text(code: u8) -> &'static str {
    match code {
        0 => "Clear",
        1..=3 => "Cloudy",
        45 | 48 => "Fog",
        51..=57 => "Drizzle",
        61..=67 => "Rain",
        71..=77 => "Snow",
        80..=82 => "Showers",
        85 | 86 => "Snow showers",
        95..=99 => "Thunder",
        _ => "Unknown",
    }
}

fn http_err<E: core::fmt::Debug>(stage: &'static str) -> impl FnOnce(E) -> String<48> {
    move |err| {
        info!("weather {stage} failed: {err:?}");
        err_msg(stage)
    }
}

fn err_msg(text: &str) -> String<48> {
    let mut s = String::new();
    let _ = s.push_str(text);
    s
}
