// SPDX-License-Identifier: MIT

//! Full-color file icons drawn from the desktop's GTK icon theme, such as a
//! theme extracted from macOS, instead of Strata's monochrome glyphs.
//!
//! Names handed to the rest of the interface carry [`PREFIX`] so they never
//! collide with bundled glyph names. Only artwork that the active theme itself
//! ships is used; inherited fallback themes would mix unrelated styles, so a
//! file type the theme lacks keeps Strata's own glyph.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::Path,
};

use gtk::{gdk, gio, glib, prelude::*};

pub const PREFIX: &str = "native:";

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static NAMES: RefCell<HashMap<String, Option<&'static str>>> = RefCell::new(HashMap::new());
    static TEXTURES: RefCell<HashMap<(String, i32), Option<gdk::Texture>>> =
        RefCell::new(HashMap::new());
}

pub fn set_enabled(enabled: bool) {
    if ENABLED.replace(enabled) != enabled {
        NAMES.with_borrow_mut(HashMap::clear);
        TEXTURES.with_borrow_mut(HashMap::clear);
    }
}

pub fn enabled() -> bool {
    ENABLED.get()
}

pub fn is_native(name: &str) -> bool {
    name.starts_with(PREFIX)
}

/// The icon for a regular folder, or `None` when the theme lacks one.
pub fn folder_icon() -> Option<&'static str> {
    if !enabled() {
        return None;
    }
    resolve_cached("inode/directory", || {
        first_theme_icon(&["folder", "inode-directory"])
    })
}

/// Desktop, Documents, Downloads and the other well-known folders.
pub fn special_folder_icon(path: &Path) -> Option<&'static str> {
    if !enabled() {
        return None;
    }
    let candidates: &[&str] = if path == glib::home_dir() {
        &["user-home", "folder-home"]
    } else {
        use glib::UserDirectory as Dir;
        let special = [
            (Dir::Desktop, &["folder-desktop", "user-desktop"][..]),
            (Dir::Documents, &["folder-documents"][..]),
            (Dir::Downloads, &["folder-downloads", "folder-download"][..]),
            (Dir::Music, &["folder-music"][..]),
            (Dir::Pictures, &["folder-pictures"][..]),
            (Dir::Videos, &["folder-videos"][..]),
            (Dir::PublicShare, &["folder-publicshare"][..]),
        ];
        special
            .into_iter()
            .find(|(dir, _)| glib::user_special_dir(*dir).is_some_and(|dir| dir == path))?
            .1
    };
    resolve_cached(&format!("path:{}", path.display()), || {
        first_theme_icon(candidates)
    })
}

/// The icon for a file name's guessed content type.
pub fn file_icon(name: &str) -> Option<&'static str> {
    if !enabled() {
        return None;
    }
    let lowered = name.to_lowercase();
    // Most types are identified by extension alone; whole names cover
    // `Makefile` and similar exact matches.
    let key = match lowered.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!("ext:{extension}"),
        _ => format!("name:{lowered}"),
    };
    resolve_cached(&key, || {
        let (content_type, _) = gio::content_type_guess(Some(name), None::<&[u8]>);
        let icon = gio::content_type_get_icon(&content_type);
        let names = icon
            .downcast_ref::<gio::ThemedIcon>()
            .map(|icon| icon.names())
            .unwrap_or_default();
        let mut candidates: Vec<String> = names.iter().map(ToString::to_string).collect();
        // shared-mime-info's generic fallbacks, should the specific names miss.
        if let Some(generic) = gio::content_type_get_generic_icon_name(&content_type) {
            candidates.push(generic.to_string());
        }
        let candidates: Vec<&str> = candidates.iter().map(String::as_str).collect();
        first_theme_icon(&candidates)
    })
}

/// Theme artwork for a [`PREFIX`]ed name, rendered for `pixel_size` logical
/// pixels at `scale_factor`.
pub fn texture(name: &str, pixel_size: i32, scale_factor: i32) -> Option<gdk::Texture> {
    let icon_name = name.strip_prefix(PREFIX)?;
    let size = pixel_size.max(1);
    let scale = scale_factor.max(1);
    let key = (icon_name.to_owned(), size * scale);
    if let Some(cached) = TEXTURES.with_borrow(|textures| textures.get(&key).cloned()) {
        return cached;
    }
    let texture = load_texture(icon_name, size, scale);
    TEXTURES.with_borrow_mut(|textures| textures.insert(key, texture.clone()));
    texture
}

fn load_texture(icon_name: &str, size: i32, scale: i32) -> Option<gdk::Texture> {
    let theme = icon_theme()?;
    let paintable = theme.lookup_icon(
        icon_name,
        &[],
        size,
        scale,
        gtk::TextDirection::Ltr,
        gtk::IconLookupFlags::empty(),
    );
    let path = paintable.file()?.path()?;
    gdk::Texture::from_filename(path).ok()
}

fn resolve_cached(
    key: &str,
    resolve: impl FnOnce() -> Option<&'static str>,
) -> Option<&'static str> {
    if let Some(cached) = NAMES.with_borrow(|names| names.get(key).copied()) {
        return cached;
    }
    let resolved = resolve();
    NAMES.with_borrow_mut(|names| names.insert(key.to_owned(), resolved));
    resolved
}

fn icon_theme() -> Option<gtk::IconTheme> {
    gdk::Display::default().map(|display| gtk::IconTheme::for_display(&display))
}

/// The first candidate the active theme ships itself, interned with [`PREFIX`].
fn first_theme_icon(candidates: &[&str]) -> Option<&'static str> {
    let theme = icon_theme()?;
    let theme_name = theme.theme_name();
    let theme_directory = format!("/{theme_name}/");
    candidates
        .iter()
        .copied()
        .find(|candidate| {
            if !theme.has_icon(candidate) {
                return false;
            }
            let paintable = theme.lookup_icon(
                candidate,
                &[],
                32,
                1,
                gtk::TextDirection::Ltr,
                gtk::IconLookupFlags::empty(),
            );
            paintable
                .file()
                .and_then(|file| file.path())
                .is_some_and(|path| path.to_string_lossy().contains(&theme_directory))
        })
        .map(intern)
}

fn intern(icon_name: &str) -> &'static str {
    thread_local! {
        static INTERNED: RefCell<HashMap<String, &'static str>> = RefCell::new(HashMap::new());
    }
    INTERNED.with_borrow_mut(|interned| {
        *interned
            .entry(icon_name.to_owned())
            .or_insert_with(|| Box::leak(format!("{PREFIX}{icon_name}").into_boxed_str()))
    })
}
