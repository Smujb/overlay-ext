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

The actual sysext builds are handled by [mkosi](https://github.com/systemd/mkosi) and the end user can configure which packages are included at `/etc/overay-ext/mkosi/`, which acts as a fully-featured mkosi directory.

When building the sysext with mkosi, the following template is used: [overlay-ext-skeleton](https://github.com/Smujb/overlay-ext-skeleton).

Some systems may require os-specific hacks. Currently only implemented for Arch Linux with a custom db path of `/usr/lib/sysimage/lib/pacman`, but can be adapted to other distributions by matching `os-release` files.
