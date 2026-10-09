# Overlay Extension Builder

```
Build system extensions for live systemd-sysupdate managed systems using mkosi

Usage: overlay-ext [OPTIONS]

Options:
  -f, --force    Whether to forceably rebuild the overlay for the booted deployment
  -h, --help     Print help
  -V, --version  Print version
```

> [!WARNING]
> This program is very early in development and will probably not work for most systems.

This program is designed to build a system extension containing additional packages built against the same snapshot as the base system. It is intended primarily for image-based systems managed by systemd-sysupdate.

The actual sysext builds are handled by [mkosi](https://github.com/systemd/mkosi) and the end user can configure which packages are included at `/etc/overlay-ext/mkosi/`, which acts as a fully-featured mkosi directory.

When building the sysext with mkosi, the following template is used: [overlay-ext-skeleton](https://github.com/Smujb/overlay-ext-skeleton).

Some systems may require os-specific hacks. Currently only implemented for Arch Linux with a custom db path of `/usr/lib/sysimage/lib/pacman`, but can be adapted to other distributions by matching `os-release` files.

## Requirements

In order to use this tool, the following requirements must be met by the system:

- The system is installed and updated via `systemd-sysupdate`, and installed `/usr` trees are located on physical partitions. Loopback files may be supported at some point, but are not currently.

- The package database is included on the image. This also means that if the package manager does not put it in `/usr`, it must be copied there.

- The image is built against a fixed snapshot, e.g. the Arch Linux Archive. This is achieved in `mkosi` using the configuration option `Snapshot=`

- In `/etc/os-release`, `VERSION_ID` is populated and updated every single image version. I may change this to `SYSEXT_LEVEL` later for better support for Fedora.

- In `/etc/os-release`, a custom field `IMAGE_SNAPSHOT` is set to a valid `mkosi` snapshot reference, either grabbed from the build itself or replicated to be what it would be if using another buildsystem (typically this would be `YYYY/MM/DD`)

Note that the system need not contain a package manager, but `mkosi` will delete package metadata by default if the package manager is not included on the final image.
