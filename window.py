import uasyncio

from config import DEBOUNCE_DELAY, WAIT_DELAY


class Window:
    def __init__(self, lock, index, open_pin, close_pin, stop_pin):
        self.index = index
        self.open_pin = open_pin
        self.close_pin = close_pin
        self.stop_pin = stop_pin
        self._percentage = 0
        self.lock = lock

    async def percentage(self):
        return self._percentage

    async def set(self, percentage):
        if percentage != 0:
            await self.open()
        else:
            await self.close()

        self._percentage = percentage

    async def init(self):
        print(f"{self.index} init start")
        # Make sure the window starts an open cycle
        await self.open()

        # Make sure the initial state is that the window is closed
        await self.close()
        print(f"{self.index} init end")

        # Window should be 0% open
        self._percentage = 0

    async def open(self):
        async with self.lock:
            for _ in range(2):
                self.open_pin.off()
                await uasyncio.sleep_ms(DEBOUNCE_DELAY)
                self.open_pin.on()
                await uasyncio.sleep_ms(DEBOUNCE_DELAY)

            await uasyncio.sleep_ms(WAIT_DELAY)

    async def close(self):
        async with self.lock:
            for _ in range(2):
                self.close_pin.off()
                await uasyncio.sleep_ms(DEBOUNCE_DELAY)
                self.close_pin.on()
                await uasyncio.sleep_ms(DEBOUNCE_DELAY)

            await uasyncio.sleep_ms(WAIT_DELAY)

    async def stop(self):
        async with self.lock:
            for _ in range(2):
                self.stop_pin.off()
                await uasyncio.sleep_ms(DEBOUNCE_DELAY)
                self.stop_pin.on()
                await uasyncio.sleep_ms(DEBOUNCE_DELAY)

            await uasyncio.sleep_ms(WAIT_DELAY)

