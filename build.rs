use std::collections::HashMap;
use std::path::Path;

fn main() {
    println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
    println!("cargo:rerun-if-changed=wifi.env");
    println!("cargo:rerun-if-env-changed=WIFI_SSID");
    println!("cargo:rerun-if-env-changed=WIFI_PASSWORD");
    println!("cargo:rerun-if-env-changed=WIFI_SSD");
    println!("cargo:rerun-if-env-changed=WEATHER_CITY");
    println!("cargo:rerun-if-env-changed=WEATHER_LAT");
    println!("cargo:rerun-if-env-changed=WEATHER_LON");

    let mut from_file = load_dotenv(Path::new("wifi.env"));

    let ssid = first_nonempty([
        std::env::var("WIFI_SSID").ok(),
        std::env::var("WIFI_SSD").ok(),
        from_file.remove("WIFI_SSID"),
        from_file.remove("WIFI_SSD"),
    ]);
    let password = first_nonempty([
        std::env::var("WIFI_PASSWORD").ok(),
        from_file.remove("WIFI_PASSWORD"),
    ]);

    let ssid = ssid.unwrap_or_else(|| "YOUR_SSID".to_string());
    let password = password.unwrap_or_else(|| "YOUR_PASSWORD".to_string());
    let city = first_nonempty([
        std::env::var("WEATHER_CITY").ok(),
        from_file.remove("WEATHER_CITY"),
    ])
    .unwrap_or_else(|| "Taipei".to_string());
    let lat = first_nonempty([
        std::env::var("WEATHER_LAT").ok(),
        from_file.remove("WEATHER_LAT"),
    ])
    .unwrap_or_else(|| "25.05".to_string());
    let lon = first_nonempty([
        std::env::var("WEATHER_LON").ok(),
        from_file.remove("WEATHER_LON"),
    ])
    .unwrap_or_else(|| "121.53".to_string());

    println!("cargo:warning=Wi-Fi SSID = {ssid}");
    println!("cargo:warning=Weather = {city} ({lat},{lon})");

    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    std::fs::write(format!("{out}/wifi_ssid.txt"), &ssid).expect("write ssid");
    std::fs::write(format!("{out}/wifi_password.txt"), &password).expect("write password");
    std::fs::write(format!("{out}/weather_city.txt"), &city).expect("write city");
    std::fs::write(format!("{out}/weather_lat.txt"), &lat).expect("write lat");
    std::fs::write(format!("{out}/weather_lon.txt"), &lon).expect("write lon");
}

fn first_nonempty<const N: usize>(values: [Option<String>; N]) -> Option<String> {
    values.into_iter().find_map(|value| {
        value.and_then(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    })
}

fn load_dotenv(path: &Path) -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };

    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'');
        map.insert(key.trim().to_string(), value.to_string());
    }
    map
}
