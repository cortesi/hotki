# mac-winops

mac-winops provides macOS window operations for Hotki, centered on working with Mission Control Spaces (listing, querying the current one, moving windows between Spaces, and optionally switching the user’s view).

Private frameworks: This crate calls Apple’s private SkyLight/CGS APIs, loaded dynamically at runtime. If an expected symbol is missing on a given macOS version, the operation returns an Unsupported error. We do not perform Dock injection, scripting additions, UI scripting, or SIP-related changes—just function calls exposed by the WindowServer.
