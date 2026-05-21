from network import fetch, fetch_text, ws_open, ws_send, ws_close

# HTTP — yielding host call: looks sync, suspends until the response arrives. Compose with gather / with_timeout exactly like sleep().

print("-> GET https://httpbin.org/json")
body = fetch_text("https://httpbin.org/json")
print(f"  {len(body)} bytes received")

print("-> GET https://httpbin.org/uuid (full response object)")
resp = fetch("https://httpbin.org/uuid")
print(f"  status field present in JSON: {'\"status\"' in resp}")

# WebSocket — streaming, so push-event pattern: bind via msg tag, drain with receive(). Top-level receive() yields the implicit module-body coro; no async def / run() wrapper needed.

print("-> WS wss://echo.websocket.org")
sock = ws_open("wss://echo.websocket.org", "echo")

while True:
    ev = receive()
    if '"type":"open"' in ev:
        ws_send(sock, "hello from edge-python")
    elif '"type":"message"' in ev:
        print(f"  echo received: {ev}")
        ws_close(sock)
        break
    elif '"type":"close"' in ev:
        break

print("done")
