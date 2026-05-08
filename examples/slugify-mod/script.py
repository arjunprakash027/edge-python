# Example Edge Python script that uses the slugify-mod native module.
#
# Run-time wiring (in the playground):
#   1. Place slugify_mod.wasm at the worker URL's directory or on a CDN.
#   2. Either import directly:
#        from "./slugify_mod.wasm" import slugify, shout, repeat_n, sum_ints
#      Or via packages.json:
#        { "imports": { "slug": "./slugify_mod.wasm" } }

from "./slugify_mod.wasm" import slugify, shout, repeat_n, sum_ints

print(slugify("Hello World"))
print(slugify("ABC 123 def!"))
print(shout("ok"))
print(repeat_n("ha", 3))
print(sum_ints([1, 2, 3, 4]))

# Errors propagate as typed Python exceptions:
try:
    print(repeat_n("nope", -1))
except ValueError as e:
    print("caught:", e)
