import json

from config import WINDOW_CONFIG
from secrets import WIFI_SSID, WIFI_PASS, MQTT_IP
from mqtt_as import MQTTClient, config
import uasyncio as asyncio

from window import Window

# Local configuration
config['ssid'] = WIFI_SSID
config['wifi_pw'] = WIFI_PASS
config['server'] = MQTT_IP

windows = {}


async def messages(client):
    async for topic, msg, retained in client.queue:
        if topic.startswith("skylight/set"):
            msg = json.loads(msg)
            await windows[msg["index"]].set(msg["percentage"])

        elif topic.startswith("skylight/get"):
            msg = json.loads(msg)
            result = {
                "index": msg["index"],
                "percentage": await windows[msg["index"]].percentage()
            }
            await client.publish('skylight/get/response', json.dumps(result), qos=0)

        else:
            print("Unknown MQTT message:", topic, msg, retained)


async def up(client):
    while True:
        await client.up.wait()
        client.up.clear()
        await client.subscribe("skylight/+", 0)


async def init():
    for index, window in WINDOW_CONFIG.items():
        window = Window(index, window['open'], window['close'], window['stop'])
        windows[index] = window

        await window.init()


async def main(client):
    asyncio.create_task(init())

    await client.connect()
    for coroutine in (up, messages):
        asyncio.create_task(coroutine(client))

    while True:
        await asyncio.sleep(5)


config["queue_len"] = 6
MQTTClient.DEBUG = True
_client = MQTTClient(config)
try:
    asyncio.run(main(_client))
finally:
    _client.close()
