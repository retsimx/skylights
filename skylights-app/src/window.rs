//! ESP32 window actuation driver, interlock, command bus, and controller tasks.
//!
//! Binds the hardware-agnostic [`skylights_core::window`] state machine to nine
//! active-low GPIO outputs (open/stop/close per window). Each pulse sequence
//! holds a single shared [`ACTUATION_LOCK`] so pulses across windows can never
//! overlap, and each window runs one interruptible controller task driven by its
//! own command channel.

use embassy_executor::task;
use embassy_futures::select::{select, Either};
use embassy_sync::channel::{Channel, Receiver};
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::Output;
use esp_hal::sync::RawMutex;
use skylights_core::window::{Actuation, PULSE_HIGH_MS, PULSE_LOW_MS, PULSE_SETTLE_MS};
use skylights_core::{Position, WindowPositioner};

/// Number of independently controlled skylight windows.
pub const WINDOW_COUNT: usize = 3;

/// Command accepted by a window controller task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCommand {
    /// Drive the window to the requested target position.
    SetTarget(Position),
    /// Stop an in-flight movement and hold the current position.
    Stop,
}

/// Per-window command channel shared between producers and the controller task.
pub type WindowChannel = Channel<RawMutex, WindowCommand, 4>;

/// One command channel per window; the seam for future MQTT ingress (SL-6).
pub static WINDOW_COMMANDS: [WindowChannel; WINDOW_COUNT] =
    [Channel::new(), Channel::new(), Channel::new()];

/// Serialises active-low pulse sequences across all windows.
static ACTUATION_LOCK: Mutex<RawMutex, ()> = Mutex::new(());

/// Last known state of a single window, shared with the MQTT task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSnapshot {
    /// Last settled (or origin) position, 0–100.
    pub percentage: u8,
    /// True while the window is travelling.
    pub moving: bool,
}

impl WindowSnapshot {
    /// The fully closed, idle snapshot used to initialise all windows.
    pub const IDLE: Self = Self {
        percentage: 0,
        moving: false,
    };
}

/// Current snapshot of every window, indexed by window id.
pub static WINDOW_STATES: Mutex<RawMutex, [WindowSnapshot; WINDOW_COUNT]> =
    Mutex::new([WindowSnapshot::IDLE; WINDOW_COUNT]);

/// Signalled whenever a window's snapshot changes.
pub static STATE_CHANGED: Signal<RawMutex, ()> = Signal::new();

/// The three actuation outputs of a single window, all active-low.
pub struct WindowPins<'d> {
    open: Output<'d>,
    stop: Output<'d>,
    close: Output<'d>,
}

impl<'d> WindowPins<'d> {
    /// Creates a pin set from the open, stop, and close outputs.
    pub const fn new(open: Output<'d>, stop: Output<'d>, close: Output<'d>) -> Self {
        Self { open, stop, close }
    }
}

/// Drives one window's actuation lines, emitting active-low pulse sequences.
pub struct WindowDriver<'d> {
    pins: WindowPins<'d>,
}

impl<'d> WindowDriver<'d> {
    /// Creates a driver over the given pin set.
    pub const fn new(pins: WindowPins<'d>) -> Self {
        Self { pins }
    }

    /// Emits the actuation pulse sequence for `actuation`.
    ///
    /// Holds [`ACTUATION_LOCK`] for the whole sequence so two windows can never
    /// pulse at once. The selected pin runs two 75 ms low / 75 ms high pulses
    /// followed by a 400 ms settle, and is left high (inactive) afterwards.
    pub async fn actuate(&mut self, actuation: Actuation) {
        let _guard = ACTUATION_LOCK.lock().await;

        let pin = match actuation {
            Actuation::Open => &mut self.pins.open,
            Actuation::Close => &mut self.pins.close,
            Actuation::Stop => &mut self.pins.stop,
        };

        pin.set_low();
        Timer::after(Duration::from_millis(PULSE_LOW_MS)).await;
        pin.set_high();
        Timer::after(Duration::from_millis(PULSE_HIGH_MS)).await;
        pin.set_low();
        Timer::after(Duration::from_millis(PULSE_LOW_MS)).await;
        pin.set_high();
        Timer::after(Duration::from_millis(PULSE_HIGH_MS)).await;
        Timer::after(Duration::from_millis(PULSE_SETTLE_MS)).await;
    }
}

/// Owns a window's positioner, driver, and command receiver and runs its loop.
pub struct WindowController<'d> {
    id: usize,
    positioner: WindowPositioner,
    driver: WindowDriver<'d>,
    receiver: Receiver<'d, RawMutex, WindowCommand, 4>,
}

impl<'d> WindowController<'d> {
    /// Creates a controller from its window id, positioner, driver, and receiver.
    pub const fn new(
        id: usize,
        positioner: WindowPositioner,
        driver: WindowDriver<'d>,
        receiver: Receiver<'d, RawMutex, WindowCommand, 4>,
    ) -> Self {
        Self {
            id,
            positioner,
            driver,
            receiver,
        }
    }

    /// Publishes this window's current snapshot and wakes the MQTT task.
    async fn record_state(&self) {
        {
            let mut states = WINDOW_STATES.lock().await;
            states[self.id] = WindowSnapshot {
                percentage: self.positioner.position().percent(),
                moving: self.positioner.is_moving(),
            };
        }
        STATE_CHANGED.signal(());
    }

    /// Consumes commands forever, driving the window and honouring interrupts.
    pub async fn run(mut self) -> ! {
        loop {
            match self.receiver.receive().await {
                WindowCommand::SetTarget(target) => self.drive_to(target).await,
                WindowCommand::Stop => {
                    self.positioner.stop(0);
                    self.record_state().await;
                }
            }
        }
    }

    /// Drives towards `target`, returning early if the positioner has no plan.
    ///
    /// A `Stop` or new `SetTarget` command arriving mid-travel interrupts the
    /// motion, pulses Stop, records the interpolated position, and either stops
    /// or continues towards the newest target (loop, not recursion).
    async fn drive_to(&mut self, mut target: Position) {
        loop {
            let Some(plan) = self.positioner.request(target) else {
                return;
            };
            self.record_state().await;
            self.driver.actuate(plan.actuation).await;

            let start = Instant::now();
            match select(
                Timer::after(Duration::from_millis(plan.travel_ms)),
                self.receiver.receive(),
            )
            .await
            {
                Either::First(()) => {
                    if plan.stop_after {
                        self.driver.actuate(Actuation::Stop).await;
                    }
                    self.positioner.finish();
                    self.record_state().await;
                    return;
                }
                Either::Second(WindowCommand::Stop) => {
                    let elapsed = start.elapsed().as_millis();
                    self.driver.actuate(Actuation::Stop).await;
                    self.positioner.stop(elapsed);
                    self.record_state().await;
                    return;
                }
                Either::Second(WindowCommand::SetTarget(next)) => {
                    let elapsed = start.elapsed().as_millis();
                    self.driver.actuate(Actuation::Stop).await;
                    self.positioner.stop(elapsed);
                    self.record_state().await;
                    target = next;
                }
            }
        }
    }
}

/// Runs the controller task for the window identified by `id`.
#[task(pool_size = 3)]
pub async fn window_task(id: usize, driver: WindowDriver<'static>) -> ! {
    let receiver = WINDOW_COMMANDS[id].receiver();
    WindowController::new(id, WindowPositioner::new(), driver, receiver)
        .run()
        .await
}
