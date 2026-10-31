//! Global shortcut to summon the app, via the XDG **GlobalShortcuts portal**.
//!
//! On Wayland there's no silent global key-grab; the portal is the sanctioned
//! mechanism. The app registers an "activate" shortcut with a preferred
//! Ctrl+Alt+O trigger — on a desktop that implements the portal (KDE Plasma 6,
//! GNOME) the user can rebind it in their global-shortcuts settings. `on_activate`
//! fires whenever the shortcut is triggered. Registers the app id with the
//! portal first; see [`crate::app_id`].
//!
//! Without the portal (an X11 session, or a desktop that doesn't implement
//! it) and with `DISPLAY` set, it falls back to grabbing Ctrl+Alt+O on the X
//! root window with `XGrabKey` (see [`x11`]). Under XWayland that grab only
//! sees keys while an X11 window has focus, so on Wayland the portal is what
//! really works. Degrades gracefully (logs, and the thread exits) when neither
//! is available, like the rest of platform.

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

/// Stable id for our one shortcut, matched on the `Activated` signal.
const SHORTCUT_ID: &str = "activate";
/// Preferred accelerator (portal syntax). The portal/desktop may override it and
/// let the user pick — `preferred_trigger` is only a hint.
const PREFERRED_TRIGGER: &str = "CTRL+ALT+o";

/// Spawn the portal listener on its own thread + runtime. `on_activate` is called
/// (on that thread) each time the shortcut fires.
pub fn spawn(on_activate: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else {
            return;
        };
        // Best-effort: a missing/unimplemented portal falls back to an X11
        // key grab when there's an X display, else means no global hotkey.
        let Err(e) = rt.block_on(run(&on_activate)) else {
            return;
        };
        eprintln!("[orrery shortcut] GlobalShortcuts portal unavailable: {e}");
        if std::env::var_os("DISPLAY").is_none_or(|d| d.is_empty()) {
            eprintln!("[orrery shortcut] no X display either; no global shortcut");
            return;
        }
        if let Err(e) = x11::grab_and_listen(&on_activate) {
            eprintln!("[orrery shortcut] X11 key grab unavailable: {e}");
        }
    });
}

async fn run(on_activate: &impl Fn()) -> ashpd::Result<()> {
    // KDE's portal rejects a GlobalShortcuts session from a caller it can't put
    // an app id to ("NotAllowed: An app id is required"). Register ours on a
    // connection of our own: registration must come before any other portal
    // call on the connection, and ashpd's shared one may already be in use by
    // gpui. A failure is logged with a hint; we still try, because a launch from
    // the desktop entry lets the portal infer the id from the systemd scope.
    let conn = zbus::Connection::session().await?;
    crate::app_id::register(&conn).await;
    let shortcuts = GlobalShortcuts::with_connection(conn).await?;
    let session = shortcuts.create_session(Default::default()).await?;

    let activate =
        NewShortcut::new(SHORTCUT_ID, "Summon Orrery").preferred_trigger(Some(PREFERRED_TRIGGER));
    // Await the bind's response so the registration completes (or its refusal
    // surfaces) before we listen. The desktop owns the final trigger, which may
    // differ from the preferred one; log it so "the hotkey does nothing" is
    // diagnosable.
    let bound = shortcuts
        .bind_shortcuts(&session, &[activate], None, Default::default())
        .await?
        .response()?;
    for s in bound.shortcuts() {
        let trigger = match s.trigger_description() {
            "" => "unassigned",
            t => t,
        };
        eprintln!("[orrery shortcut] bound '{}' → {trigger}", s.id());
    }

    // The stream borrows nothing of `session`, but `session` must outlive it:
    // dropping the session ends the registration. Both stay in scope here.
    let mut activated = shortcuts.receive_activated().await?;
    while let Some(activation) = activated.next().await {
        if activation.shortcut_id() == SHORTCUT_ID {
            on_activate();
        }
    }
    Ok(())
}

/// The X11 fallback: a passive `XGrabKey` of Ctrl+Alt+O on the root window,
/// over x11rb's pure-Rust connection (no libxcb).
mod x11 {
    use std::error::Error;

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, GrabMode, ModMask};
    use x11rb::protocol::Event;

    /// `XK_o` / `XK_O`: either keysym on a keycode makes it the O key.
    const KEYSYMS_O: [u32; 2] = [0x006f, 0x004f];
    /// `XK_Num_Lock`, to find which modifier bit NumLock sets.
    const KEYSYM_NUM_LOCK: u32 = 0xff7f;
    /// Key repeat while held arrives as extra presses; ignore presses this
    /// soon (ms, server time) after the last one.
    const REPEAT_MS: u32 = 300;

    /// The first keycode in `min_keycode..` whose keysym row (of `per` keysyms,
    /// as GetKeyboardMapping returns them) contains any of `targets`.
    pub(super) fn find_keycode(
        min_keycode: u8,
        per: u8,
        keysyms: &[u32],
        targets: &[u32],
    ) -> Option<u8> {
        if per == 0 {
            return None;
        }
        keysyms
            .chunks(per as usize)
            .position(|row| row.iter().any(|k| targets.contains(k)))
            .and_then(|i| u8::try_from(min_keycode as usize + i).ok())
    }

    /// The modifier bit (Shift=0 … Mod5=7, as a mask) that `keycode` is
    /// mapped to in a GetModifierMapping reply, if any.
    pub(super) fn modifier_mask(modmap: &[u8], keycode: u8) -> Option<u16> {
        let per = modmap.len() / 8;
        if per == 0 || keycode == 0 {
            return None;
        }
        modmap
            .chunks(per)
            .position(|row| row.contains(&keycode))
            .map(|bit| 1u16 << bit)
    }

    /// `base` plus every combination of CapsLock and NumLock: a passive grab
    /// matches modifiers exactly, so without these the hotkey dies whenever
    /// either lock is on.
    pub(super) fn lock_variants(base: u16, num_lock: u16) -> Vec<u16> {
        let caps = u16::from(ModMask::LOCK);
        let mut out = vec![base, base | caps];
        if num_lock != 0 && num_lock != caps {
            out.extend([base | num_lock, base | caps | num_lock]);
        }
        out
    }

    /// Grab the key on the default screen's root window and call `on_activate`
    /// for each press. Returns an error if the connection or grab fails (e.g.
    /// another client already holds Ctrl+Alt+O); otherwise runs until the X
    /// connection closes.
    pub(super) fn grab_and_listen(on_activate: &impl Fn()) -> Result<(), Box<dyn Error>> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let map = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        let keycode = find_keycode(min, map.keysyms_per_keycode, &map.keysyms, &KEYSYMS_O)
            .ok_or("no keycode for the O key")?;
        let num_lock = find_keycode(
            min,
            map.keysyms_per_keycode,
            &map.keysyms,
            &[KEYSYM_NUM_LOCK],
        )
        .and_then(|kc| {
            let modmap = conn.get_modifier_mapping().ok()?.reply().ok()?;
            modifier_mask(&modmap.keycodes, kc)
        })
        // Mod2 is NumLock on practically every layout.
        .unwrap_or(u16::from(ModMask::M2));

        let base = u16::from(ModMask::CONTROL | ModMask::M1);
        for mods in lock_variants(base, num_lock) {
            conn.grab_key(
                false,
                root,
                ModMask::from(mods),
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )?
            .check()?;
        }
        eprintln!("[orrery shortcut] bound 'activate' → Ctrl+Alt+O (X11 key grab)");

        let mut last: Option<u32> = None;
        loop {
            if let Event::KeyPress(e) = conn.wait_for_event()? {
                if e.detail != keycode {
                    continue;
                }
                if last.is_some_and(|t| e.time.wrapping_sub(t) < REPEAT_MS) {
                    last = Some(e.time);
                    continue;
                }
                last = Some(e.time);
                on_activate();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::x11::*;

    #[test]
    fn finds_the_keycode_by_keysym_row() {
        // Rows of 2 keysyms from keycode 8: 8 → a/A, 9 → o/O, 10 → p/P.
        let keysyms = [0x61, 0x41, 0x6f, 0x4f, 0x70, 0x50];
        assert_eq!(find_keycode(8, 2, &keysyms, &[0x6f, 0x4f]), Some(9));
        assert_eq!(find_keycode(8, 2, &keysyms, &[0xff7f]), None);
        assert_eq!(find_keycode(8, 0, &keysyms, &[0x6f]), None);
    }

    #[test]
    fn modifier_mask_is_the_row_bit() {
        // 2 keycodes per modifier; keycode 77 is on Mod2 (row 4).
        let mut modmap = [0u8; 16];
        modmap[8] = 77;
        assert_eq!(modifier_mask(&modmap, 77), Some(1 << 4));
        assert_eq!(modifier_mask(&modmap, 78), None);
        assert_eq!(modifier_mask(&modmap, 0), None);
    }

    #[test]
    fn grabs_every_lock_combination() {
        let (ctrl_alt, lock, mod2) = (0x0c, 0x02, 0x10);
        assert_eq!(
            lock_variants(ctrl_alt, mod2),
            vec![
                ctrl_alt,
                ctrl_alt | lock,
                ctrl_alt | mod2,
                ctrl_alt | lock | mod2
            ]
        );
        // NumLock unmapped: just the CapsLock variants.
        assert_eq!(lock_variants(ctrl_alt, 0), vec![ctrl_alt, ctrl_alt | lock]);
    }

    /// Grabs and releases the real key on `DISPLAY` (XWayland counts). Run
    /// with `cargo test -p orrery-platform -- --ignored x11_grab`.
    #[test]
    #[ignore = "needs an X display"]
    fn x11_grab_smoke() {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{ConnectionExt, GrabMode, ModMask};
        let (conn, screen) = x11rb::connect(None).expect("X display");
        let root = conn.setup().roots[screen].root;
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let map = conn
            .get_keyboard_mapping(min, max - min + 1)
            .unwrap()
            .reply()
            .unwrap();
        let kc = find_keycode(min, map.keysyms_per_keycode, &map.keysyms, &[0x6f, 0x4f])
            .expect("O keycode");
        let modmap = conn.get_modifier_mapping().unwrap().reply().unwrap();
        let nl = find_keycode(min, map.keysyms_per_keycode, &map.keysyms, &[0xff7f])
            .and_then(|k| modifier_mask(&modmap.keycodes, k));
        eprintln!("O keycode {kc}, NumLock mask {nl:?}");
        let base = u16::from(ModMask::CONTROL | ModMask::M1);
        for mods in lock_variants(base, nl.unwrap_or(0x10)) {
            conn.grab_key(
                false,
                root,
                ModMask::from(mods),
                kc,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .unwrap()
            .check()
            .expect("grab");
            conn.ungrab_key(kc, root, ModMask::from(mods))
                .unwrap()
                .check()
                .unwrap();
        }
    }
}
