use std::collections::HashSet;

use glam::Vec2;
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

/// Keyboard and mouse state, accumulated between frames by the shell.
#[derive(Default)]
pub struct Input {
    pressed: HashSet<KeyCode>,
    just_pressed: HashSet<KeyCode>,
    mouse_pressed: HashSet<MouseButton>,
    mouse_just_pressed: HashSet<MouseButton>,
    pub(crate) mouse_delta: Vec2,
    pub(crate) scroll: f32,
    pub(crate) captured: bool,
}

impl Input {
    pub fn is_down(&self, key: KeyCode) -> bool {
        self.pressed.contains(&key)
    }

    /// True only on the frame the key transitioned from up to down.
    pub fn is_just_pressed(&self, key: KeyCode) -> bool {
        self.just_pressed.contains(&key)
    }

    pub fn is_mouse_down(&self, button: MouseButton) -> bool {
        self.mouse_pressed.contains(&button)
    }

    /// True only on the frame the button went down. Note: the click that
    /// captures the cursor is consumed by the shell and never reported.
    pub fn is_mouse_just_pressed(&self, button: MouseButton) -> bool {
        self.mouse_just_pressed.contains(&button)
    }

    /// Mouse movement since the last frame, in device pixels.
    /// Only accumulates while the cursor is captured.
    pub fn mouse_delta(&self) -> Vec2 {
        self.mouse_delta
    }

    /// Scroll wheel movement since the last frame, in lines (up = positive).
    pub fn scroll_delta(&self) -> f32 {
        self.scroll
    }

    /// True while the cursor is grabbed for mouse-look.
    pub fn is_captured(&self) -> bool {
        self.captured
    }

    pub(crate) fn set_key(&mut self, key: KeyCode, down: bool) {
        if down {
            if self.pressed.insert(key) {
                self.just_pressed.insert(key);
            }
        } else {
            self.pressed.remove(&key);
        }
    }

    pub(crate) fn set_mouse_button(&mut self, button: MouseButton, down: bool) {
        if down {
            if self.mouse_pressed.insert(button) {
                self.mouse_just_pressed.insert(button);
            }
        } else {
            self.mouse_pressed.remove(&button);
        }
    }

    pub(crate) fn add_scroll(&mut self, lines: f32) {
        self.scroll += lines;
    }

    pub(crate) fn end_frame(&mut self) {
        self.mouse_delta = Vec2::ZERO;
        self.scroll = 0.0;
        self.just_pressed.clear();
        self.mouse_just_pressed.clear();
    }
}
