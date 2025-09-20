# CTI View

This is a simple viewer for the [CTI image file format](https://github.com/bezverec/cti).

---

## Build from Source

### Prerequisites
1. install [Git](https://git-scm.com/)
2. install [**Rust** (stable)](https://www.rust-lang.org/tools/install) and Cargo

### Compilation (Windows)   
1. ```bash
   git clone https://github.com/bezverec/cti-view.git
   ```
2. ```bash
   cd cti
   ```
3. ```bash
   $env:RUSTFLAGS="-C target-cpu=native"; cargo build --release
   # binary will be in: .\cti\target\release\cti-view.exe
   ```
---
## Screenshot

<p align="center">
<img width="802" height="632" alt="cti-view" src="https://github.com/user-attachments/assets/ab8f2b1a-6b0a-4a6d-b839-65a1b0f7c50f" />
</p>

---
## AI generated code disclosure

The code is AI generated using ChatGPT model 5.
