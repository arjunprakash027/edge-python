from dom import (
    query, set_text, create_element, 
    append_child, add_class,
)

set_text(query("#app"), "Hello, World!")

ul = query("#list")
for i in range(5):
    li = create_element("li")
    set_text(li, "row " + str(i + 1))
    add_class(li, "fresh")
    append_child(ul, li)
