from storage import (
    local_get, local_set, local_keys,
    session_set, session_get,
    idb_open, idb_put, idb_get, idb_keys, idb_close,
)

# localStorage / sessionStorage, sync; getItem returns None when the key is missing.

print("-> localStorage")
local_set("theme", "dark")
local_set("user", "ada")
print(f"  theme = {local_get('theme')}")
print(f"  keys  = {local_keys()}")

print("-> sessionStorage")
session_set("cart", "[1,2,3]")
print(f"  cart = {session_get('cart')}")

# IndexedDB, yielding host calls: looks sync, suspends until the transaction settles. Compose with gather / with_timeout exactly like fetch().

print("-> IndexedDB")
db = idb_open("notes", 1, '{"stores":["items"]}')
idb_put(db, "items", "1", '{"title":"hello","ts":1234}')
idb_put(db, "items", "2", '{"title":"world","ts":5678}')
print(f"  item-1 = {idb_get(db, 'items', '1')}")
print(f"  keys   = {idb_keys(db, 'items')}")
idb_close(db)

print("done")
