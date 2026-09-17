from pathlib import Path

main = Path("src/main.rs")
source = main.read_text()
source = source.replace("    command: CommandKind,\n", "", 1)
source = source.replace("        command,\n        openapi: required(\"--openapi\")?,\n", "        openapi: required(\"--openapi\")?,\n", 1)
main.write_text(source)

test = Path("tests/rust_surface.rs")
source = test.read_text()
source = source.replace(
    "    generate, ApiInventory, Bindings, GenerateInput, OpenApi, Runtime, SdkDefinition,\n",
    "    generate, ApiInventory, GenerateInput, OpenApi, Runtime,\n",
    1,
)
test.write_text(source)
