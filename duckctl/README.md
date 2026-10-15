# duckctl

Talk to a [Microduck](https://github.com/pollen-robotics/microduck) robot from a laptop, over
Bluetooth LE — no network, no ssh. Linux, macOS and Windows.

```bash
uvx duckctl scan
uvx duckctl --name <robot-name> info
```

Or install it once with `uv tool install duckctl` (or `pipx install duckctl`) and drop the `uvx`.

Every command is in [`docs/robot/duckctl.md`](https://github.com/pollen-robotics/microduck/blob/main/docs/robot/duckctl.md).
