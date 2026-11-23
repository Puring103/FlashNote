# NoteHarbor GPUI patch

Based on crates.io gpui 0.2.2 (Apache-2.0). Only runtime sources and build resources are vendored.

Adds `Window::set_visual_appearance(scale, opacity, normalized_anchor)` backed by Windows DirectComposition. The transform affects the complete rendered surface while keeping the HWND, GPU surface, layout, and input coordinates unchanged. Callers must restore scale/opacity to 1 at animation completion. Other backends and the DirectComposition-disabled backend return `false`.

Windows caption hit testing uses the pointer position from the current native message. Resize edges take precedence over caption controls. The window control callback accepts a client-relative position on each backend.

The Windows backend explicitly captures the pointer on custom caption presses and moves those windows while the left button is held.

Linux falls back to X11 when initial Wayland setup fails and an X11 display is available.

Example and test binary declarations were removed from Cargo.toml because the checkout only includes runtime sources and build resources.

Cargo.lock retains libc 0.2.186 for the Linux build because gpui_http_client pulls in xattr 0.2.3, which uses libc::ENOATTR.
