from storage import (
    local_get, local_set, local_keys,
    session_set, session_get,
    idb_open, idb_put, idb_get, idb_keys, idb_close,
)

# localStorage / sessionStorage — sync; getItem returns None when the key is missing.

print("-> localStorage")
local_set("theme", "dark")
local_set("user", "ada")
theme = local_get("theme")
keys = local_keys()
print(f"  theme = {theme}")
print(f"  keys  = {keys}")

print("-> sessionStorage")
session_set("cart", "[1,2,3]")
cart = session_get("cart")
print(f"  cart = {cart}")

# IndexedDB — yielding host calls: looks sync, suspends until the transaction settles. Compose with gather / with_timeout exactly like fetch().

print("-> IndexedDB")
db = idb_open("notes", 1, '{"stores":["items"]}')
idb_put(db, "items", "1", '{"title":"hello","ts":1234}')
idb_put(db, "items", "2", '{"title":"world","ts":5678}')
item = idb_get(db, "items", "1")
all_keys = idb_keys(db, "items")
print(f"  item-1 = {item}")
print(f"  keys   = {all_keys}")
idb_close(db)

print("done")
