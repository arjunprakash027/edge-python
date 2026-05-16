"""
Importing a wasm-pdk module from Edge Python.
    Build slugify_mod.wasm `cargo build --release --target wasm32-unknown-unknown -p slugify-mod`
"""

from "./slugify_mod.wasm" import slugify, shout, repeat_n, sum_ints

print(slugify("Hello World"))
print(slugify("ABC 123 def!"))
print(shout("ok"))
print(repeat_n("ha", 3))
print(sum_ints([1, 2, 3, 4]))

# Errors raised inside the module surface as typed Python exceptions.
try:
    print(repeat_n("nope", -1))
except ValueError as e:
    print("caught:", e)
