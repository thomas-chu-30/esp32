//! T-Display 實體鍵：GPIO0 左（切頁）、GPIO35 右（刷新）。

use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer};
use esp_hal::gpio::Input;

/// 左鍵切畫面、右鍵立刻重抓、逾時則當刷新。
#[derive(Clone, Copy, Debug)]
pub enum Event {
    TogglePage,
    Refresh,
}

pub static EVENTS: Channel<CriticalSectionRawMutex, Event, 4> = Channel::new();

const DEBOUNCE: Duration = Duration::from_millis(30);

async fn wait_press(button: &mut Input<'_>) {
    loop {
        if button.is_low() {
            Timer::after(DEBOUNCE).await;
            if button.is_low() {
                return;
            }
        }
        button.wait_for_falling_edge().await;
        Timer::after(DEBOUNCE).await;
        if button.is_low() {
            return;
        }
    }
}

async fn wait_release(button: &mut Input<'_>) {
    loop {
        if button.is_high() {
            Timer::after(DEBOUNCE).await;
            if button.is_high() {
                return;
            }
        }
        button.wait_for_rising_edge().await;
        Timer::after(DEBOUNCE).await;
        if button.is_high() {
            return;
        }
    }
}

/// 監聽單顆 active-low 按鍵，去抖後送出事件。
#[embassy_executor::task(pool_size = 2)]
pub async fn watch(mut pin: Input<'static>, event: Event) {
    loop {
        wait_press(&mut pin).await;
        let _ = EVENTS.try_send(event);
        wait_release(&mut pin).await;
    }
}

/// 等到按鍵，或 `idle` 過了還沒按。
pub async fn wait(idle: Duration) -> Option<Event> {
    match select(EVENTS.receive(), Timer::after(idle)).await {
        Either::First(event) => Some(event),
        Either::Second(()) => None,
    }
}

/// 抓天氣期間若按了左鍵，抓完後仍要切頁；刷新事件已處理則丟棄。
pub fn drain_toggles() -> usize {
    let mut toggles = 0;
    while let Ok(event) = EVENTS.try_receive() {
        if matches!(event, Event::TogglePage) {
            toggles += 1;
        }
    }
    toggles
}
