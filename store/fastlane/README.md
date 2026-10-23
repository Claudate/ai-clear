fastlane documentation
----

# Installation

Make sure you have the latest version of the Xcode command line tools installed:

```sh
xcode-select --install
```

For _fastlane_ installation instructions, see [Installing _fastlane_](https://docs.fastlane.tools/#installing-fastlane)

# Available Actions

## iOS

### ios preflight

```sh
[bundle exec] fastlane ios preflight
```

检查上架所需的一切是否就位

### ios metadata

```sh
[bundle exec] fastlane ios metadata
```

同步商店页文案与截图（不传二进制）

### ios upload

```sh
[bundle exec] fastlane ios upload
```

上传已签名的 .pkg（等价于 Xcode Organizer 的 Distribute）

### ios release

```sh
[bundle exec] fastlane ios release
```

完整上架：传二进制 → 等处理 → 同步商店页

----

This README.md is auto-generated and will be re-generated every time [_fastlane_](https://fastlane.tools) is run.

More information about _fastlane_ can be found on [fastlane.tools](https://fastlane.tools).

The documentation of _fastlane_ can be found on [docs.fastlane.tools](https://docs.fastlane.tools).
