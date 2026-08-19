use crate::{
    backend::Backend,
    buffer::{Buffer, Cell},
    terminal::Config,
};
use helix_view::graphics::{CursorKind, Rect};
use std::io;

/// A headless backend that renders into an in-memory [`Buffer`] instead of a
/// real terminal, read out by the browser-facing glue layer after each frame.
#[derive(Debug)]
pub struct WasmBackend {
    width: u16,
    height: u16,
    buffer: Buffer,
    cursor: bool,
    cursor_kind: CursorKind,
    pos: (u16, u16),
}

impl WasmBackend {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            buffer: Buffer::empty(Rect::new(0, 0, width, height)),
            cursor: false,
            cursor_kind: CursorKind::Hidden,
            pos: (0, 0),
        }
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn cursor_position(&self) -> Option<(u16, u16)> {
        self.cursor.then_some(self.pos)
    }

    pub fn cursor_kind(&self) -> CursorKind {
        self.cursor_kind
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.buffer.resize(Rect::new(0, 0, width, height));
        self.width = width;
        self.height = height;
    }
}

impl Backend for WasmBackend {
    fn claim(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn reconfigure(&mut self, _config: Config) -> Result<(), io::Error> {
        Ok(())
    }

    fn restore(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn draw<'a, I>(&mut self, content: I) -> Result<(), io::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, c) in content {
            self.buffer[(x, y)] = c.clone();
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), io::Error> {
        self.cursor = false;
        Ok(())
    }

    fn show_cursor(&mut self, kind: CursorKind) -> Result<(), io::Error> {
        self.cursor = true;
        self.cursor_kind = kind;
        Ok(())
    }

    fn set_cursor(&mut self, x: u16, y: u16) -> Result<(), io::Error> {
        self.pos = (x, y);
        Ok(())
    }

    fn clear(&mut self) -> Result<(), io::Error> {
        self.buffer.reset();
        Ok(())
    }

    fn start_sync(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn end_sync(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn size(&self) -> Result<Rect, io::Error> {
        Ok(Rect::new(0, 0, self.width, self.height))
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn supports_true_color(&self) -> bool {
        true
    }

    fn get_theme_mode(&self) -> Option<helix_view::theme::Mode> {
        None
    }

    fn set_background_color(&mut self, _color: Option<helix_view::theme::Color>) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_writes_into_buffer() {
        let mut backend = WasmBackend::new(4, 1);
        let cell = Cell::default();
        Backend::draw(&mut backend, std::iter::once((0u16, 0u16, &cell))).unwrap();
        assert_eq!(backend.buffer().content.len(), 4);
    }

    #[test]
    fn resize_updates_buffer_area() {
        let mut backend = WasmBackend::new(4, 1);
        backend.resize(10, 5);
        assert_eq!(backend.size().unwrap(), Rect::new(0, 0, 10, 5));
    }
}
