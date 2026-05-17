from dom import (
    query, create_element, append_child, 
    set_text, set_attribute, add_class,
)

palette = [
    ("Claude", "#d97757"),
    ("Slate", "#1f2937"),
    ("Sky", "#0ea5e9"),
    ("Lime", "#84cc16"),
    ("Rose", "#f43f5e"),
    ("Amber", "#f59e0b"),
]

grid = query("#palette")
for name, hex_code in palette:
    swatch = create_element("div")
    add_class(swatch, "swatch")
    set_attribute(swatch, "style", "background:" + hex_code)

    label = create_element("span")
    add_class(label, "label")
    set_text(label, name + " " + hex_code)

    append_child(swatch, label)
    append_child(grid, swatch)
