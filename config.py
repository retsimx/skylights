from machine import Pin

WINDOW_CONFIG = {
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
