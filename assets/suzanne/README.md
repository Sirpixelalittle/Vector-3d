# Suzanne test asset

`Suzanne.gltf` / `Suzanne.bin` come from the Khronos
[glTF-Sample-Assets](https://github.com/KhronosGroup/glTF-Sample-Assets)
repository (`Models/Suzanne`). The Suzanne model is © the Blender
Foundation, distributed there under CC-BY 4.0. Used here purely as
converter/renderer test data. Texture files are intentionally not
vendored — the converter never loads images.

`suzanne.vec` is generated output:

```
cargo run -p vex-convert -- assets/suzanne/Suzanne.gltf -o assets/suzanne/suzanne.vec
```
