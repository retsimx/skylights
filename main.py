from machine import Pin
from utime import sleep_ms

windows = {
    1: {
        'open': Pin(2, Pin.OUT, value=1),
        'stop': Pin(4, Pin.OUT, value=1),
        'close': Pin(16, Pin.OUT, value=1),
        'duration': 71
    },
    2: {
        'open': Pin(19, Pin.OUT, value=1),
        'stop': Pin(5, Pin.OUT, value=1),
        'close': Pin(18, Pin.OUT, value=1),
        'duration': 71
    },
    3: {
        'open': Pin(21, Pin.OUT, value=1),
        'stop': Pin(22, Pin.OUT, value=1),
        'close': Pin(23, Pin.OUT, value=1),
        'duration': 71
    }
}


DEBOUNCE_DELAY = 60
WAIT_DELAY = 300


def open(window):
    for _ in range(2):
        windows[window]['open'].off()
        sleep_ms(DEBOUNCE_DELAY)
        windows[window]['open'].on()
        sleep_ms(DEBOUNCE_DELAY)

    sleep_ms(WAIT_DELAY)


def close(window):
    for _ in range(2):
        windows[window]['close'].off()
        sleep_ms(DEBOUNCE_DELAY)
        windows[window]['close'].on()
        sleep_ms(DEBOUNCE_DELAY)

    sleep_ms(WAIT_DELAY)


def stop(window):
    for _ in range(2):
        windows[window]['stop'].off()
        sleep_ms(DEBOUNCE_DELAY)
        windows[window]['stop'].on()
        sleep_ms(DEBOUNCE_DELAY)

    sleep_ms(WAIT_DELAY)


def init():
    # Make sure the windows start an open cycle
    for window in windows.keys():
        open(window)

    # Make sure the initial state is that windows are closed
    for window in windows.keys():
        close(window)


init()
