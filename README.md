# Overlay Extension Builder

> [!WARNING]
> This program is very early in development and will probably not work for most systems.

This program is designed to build a system extension containing additional packages built against the same snapshot as the base system. It is intended primarily for image-based systems managed by systemd-sysupdate.

The actual sysext builds are handled by [mkosi](https://github.com/systemd/mkosi) and the end user can configure which packages are included at `/etc/overay-ext/mkosi/`, which acts as a fully-featured mkosi directory.
