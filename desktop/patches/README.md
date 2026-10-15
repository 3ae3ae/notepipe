# Reviewed GPUI source patches

`scripts/sources.py --locked` checks these patches against the pinned source
revisions, then applies them to the generated `.deps` worktrees. Repeating source
preparation preserves the same result. Edits inside or outside a patch are
checked in a disposable copy and rejected without changing the original files.
Developer checkouts are never patched.

- `zed-input-accessibility.patch` adds an accessibility-only focus delegate for
  controls whose semantic frame and editing engine are separate elements. It
  does not register another keyboard dispatch target or Tab stop.
- `component-input-accessibility.patch` connects each input's semantic frame to
  the actual editor focus, exposes ordinary text runs and directional selection,
  handles native selection updates, reports read-only/disabled states, and skips
  disabled inputs in Tab navigation. Masked/password values remain excluded.
- `zed-macos-window.patch` forwards native window focus to AccessKit, initializes
  the adapter from the window's existing key state, and supplies an asynchronous
  AppKit fallback when GPUI has unserved frame demand. The former independent
  16 ms fallback also drew beside a working display link; a foreground trace
  captured two ordinary submissions followed by a transaction draw stalled in
  `nextDrawable`. Demand generations now let each callback consume only prior
  requests, preserving new requests made inside it. The fallback checks actual
  completed callbacks, rather than trusting a display-link running flag. Its
  32 ms deadline is anchored to the request or latest completed callback, so
  stopped or unavailable display links retain a redraw path after one interval
  without progress. A working display link keeps its own refresh rate. Delayed
  invalidation still yields the main thread; temporarily unavailable callbacks
  retain a retry, and closing cancels pending demand. Pure state-machine tests
  and executor tests cover primary consumption, demand inside a frame, takeover
  after progress stops, coalescing, unavailable callbacks, reentrancy and close.
  The Metal submission order and timeout settings are unchanged.
- `component-tooltip-lifecycle.patch` dismisses a window's managed tooltip
  before mouse or keyboard navigation can remove its trigger. It also cancels
  delayed tooltips, while preserving normal hovering and other windows.
- `component-switch-geometry.patch` adds an optional absolute or rem track
  height. The track width, thumb, inset and radius keep their existing
  proportions, and the existing switch owns input, focus and thumb motion.
  Without an override, its original pixel sizes are unchanged. The app's
  shared switch adapter opts into 20/14 rem, preserving 36×20 at 100% and
  scaling to 72×40 at 200%. The opt-in `reference_geometry()` used by
  processing switches instead uses a 52×32 track, a 24px selected / 16px
  unselected thumb and a 2px unselected outline. These measurements scale
  together with the requested track height; ordinary switches retain the
  proportional mode above. Input, focus and reduced-motion behavior stay shared.
- `component-button-text-size.patch` adds an opt-in `label_text_size` to the
  internal button label. Settings use it for their 16px reference typography;
  callers without an override retain the original size behavior.
- `zed-metal-frame-lifetime.patch` scopes Metal drawing to one autorelease pool
  per frame. `CAMetalLayer::nextDrawable` returns an autoreleased object from a
  finite pool; keeping it alive across successive display callbacks can delay
  reuse. The drawable, encoding and submission stay inside that scope. This
  does not change frame scheduling, transaction presentation or Metal timeouts.
- `zed-metal-present-stages.patch` adds opt-in profiler spans for the window
  lock, drawable acquisition, encoding, submission, transaction presentation
  and autorelease drain. The fixed-size timing accumulator is attached to the
  existing present event; JSON formatting and file output stay in the desktop
  app's background collector. Non-profiler builds omit clock and storage work.
  These spans locate stalls without changing native presentation order.

When upgrading the locked revisions, review upstream changes before adjusting a
patch. Do not edit `.deps` to fix an application build. The preparer deliberately
stops if a patch no longer applies or finds unrecognized source modifications.

Validation:

```sh
python3 -m unittest discover -s desktop/scripts -p test_sources.py
python3 desktop/scripts/sources.py --locked
cargo check --manifest-path desktop/Cargo.toml
```

The patches contain regression tests for semantic focus delegation, an actual
accessibility-enabled input draw with one Tab stop, Unicode and
line-break offsets, reversed selection, and rejecting stale native text ranges.
Native AX/VoiceOver verification still requires running the desktop application.
