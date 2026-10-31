# Rust embeds source paths in panic locations. Replace the home directory prefix with a
# same-length placeholder so the wasm stays byte-for-byte valid (offsets unchanged).
import os, sys
home = (os.path.expanduser("~") + "/").encode()
placeholder = (b"/build/" + b"_" * len(home))[: len(home) - 1] + b"/"
for path in sys.argv[1:]:
    data = open(path, "rb").read()
    open(path, "wb").write(data.replace(home, placeholder))
    print(path, data.count(home), "paths replaced")
