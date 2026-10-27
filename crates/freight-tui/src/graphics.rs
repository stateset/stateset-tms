//! Bounded direct RGB transport, including quiet tmux placeholder placement.
use crate::evidence::Pixels;
use base64::{engine::general_purpose::STANDARD, Engine};
use ratatui::layout::Rect;
use std::io::{self, Write};

#[path = "graphics_diacritics.rs"]
mod diacritics;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Auto,
    Kitty,
    Tmux,
    Off,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Direct,
    Tmux,
}
impl Transport {
    pub fn command(self, out: &mut impl Write, body: &str) -> io::Result<()> {
        // Control data and base64 payloads are constructed locally. Never let a
        // future caller insert another terminal control sequence in the body.
        if !body.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid graphics command",
            ));
        }
        match self {
            Self::Direct => write!(out, "\x1b_G{body}\x1b\\"),
            Self::Tmux => write!(
                out,
                "\x1bPtmux;\x1b\x1b7\x1b\x1b_G{body}\x1b\x1b\\\x1b\x1b8\x1b\\"
            ),
        }
    }
}
pub struct Renderer {
    pub id: u32,
    transport: Transport,
    current: Option<(String, Rect, (u32, u32))>,
    cell: (u32, u32),
}
impl Renderer {
    pub fn new(id: u32, transport: Transport) -> Self {
        Self {
            id,
            transport,
            current: None,
            cell: (8, 16),
        }
    }
    pub fn set_cell(&mut self, cell: (u32, u32)) {
        self.cell = (cell.0.max(1), cell.1.max(1));
    }
    pub fn invalidate(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.current.take().is_some() {
            delete(out, self.id, true, self.transport)?;
            out.flush()?;
        }
        Ok(())
    }
    pub fn update(
        &mut self,
        out: &mut impl Write,
        image: Option<(&Pixels, Rect)>,
    ) -> io::Result<()> {
        let key = image.map(|(pixels, area)| (pixels.digest.clone(), area, self.cell));
        if key == self.current {
            return Ok(());
        }
        let same_pixels = key
            .as_ref()
            .zip(self.current.as_ref())
            .is_some_and(|(a, b)| a.0 == b.0);
        if self.current.is_some() {
            delete(out, self.id, !same_pixels, self.transport)?;
        }
        if let Some((pixels, area)) = image {
            // Defensive boundary even if a future caller bypasses HTTP validation.
            if area.is_empty()
                || pixels.width == 0
                || pixels.height == 0
                || pixels.width > crate::evidence::SIDE
                || pixels.height > crate::evidence::SIDE
                || pixels.rgb.len() != pixels.width as usize * pixels.height as usize * 3
                || (self.transport == Transport::Tmux && (area.width > 256 || area.height > 256))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid preview dimensions",
                ));
            }
            if !same_pixels {
                // 3072 raw bytes become <=4096 base64 bytes, aligned to 4 bytes.
                let chunks = pixels.compressed.chunks(3072);
                let count = chunks.len();
                for (n, chunk) in chunks.enumerate() {
                    let more = u8::from(n + 1 < count);
                    let header = if n == 0 {
                        format!(
                            "a=t,t=d,f=24,o=z,s={},v={},i={},q=2,m={more};",
                            pixels.width, pixels.height, self.id
                        )
                    } else {
                        format!("q=2,m={more};")
                    };
                    self.transport
                        .command(out, &(header + &STANDARD.encode(chunk)))?;
                }
            }
            if self.transport == Transport::Tmux {
                // No physical cursor placement escapes bypass the pane. The
                // normal text grid refers to this single owned virtual image.
                self.transport.command(
                    out,
                    &format!(
                        "a=p,U=1,i={},p=1,c={},r={},C=1,q=2;",
                        self.id, area.width, area.height
                    ),
                )?;
            } else {
                // Let the terminal derive the other axis from source aspect.
                // Ghostty 1.3.1 stretches independently rounded c/r rectangles.
                // Choose the limiting axis using actual cell pixels, including
                // extremely tall/wide sources and one-cell viewports.
                let width = u64::from(area.width) * u64::from(self.cell.0);
                let height = u64::from(area.height) * u64::from(self.cell.1);
                let dimension =
                    if width * u64::from(pixels.height) <= height * u64::from(pixels.width) {
                        format!("c={}", area.width)
                    } else {
                        format!("r={}", area.height)
                    };
                write!(
                    out,
                    "\x1b7\x1b[{};{}H\x1b_Ga=p,i={},p=1,{},C=1,q=2;\x1b\\\x1b8",
                    area.y + 1,
                    area.x + 1,
                    self.id,
                    dimension
                )?;
            }
        }
        self.current = key;
        out.flush()
    }
}
pub fn delete(out: &mut impl Write, id: u32, free: bool, transport: Transport) -> io::Result<()> {
    transport.command(
        out,
        &format!("a=d,d={},i={id},q=2;", if free { "I" } else { "i" }),
    )
}

pub fn placeholder_symbol(id: u32, row: u16, column: u16) -> Option<String> {
    Some(format!(
        "\u{10eeee}{}{}{}",
        diacritics::MARKS.get(usize::from(row))?,
        diacritics::MARKS.get(usize::from(column))?,
        diacritics::MARKS[(id >> 24) as usize]
    ))
}
pub fn placeholders(frame: &mut ratatui::Frame, id: u32, rect: Rect) {
    let color = ratatui::style::Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8);
    for row in 0..rect.height.min(256) {
        for col in 0..rect.width.min(256) {
            if let Some(symbol) = placeholder_symbol(id, row, col) {
                frame.buffer_mut()[(rect.x + col, rect.y + row)]
                    .set_symbol(&symbol)
                    .set_fg(color);
            }
        }
    }
}
pub fn fit(pixels: &Pixels, area: Rect, cell: (u32, u32)) -> Rect {
    if area.is_empty() {
        return area;
    }
    let cw = cell.0.max(1) as f64;
    let ch = cell.1.max(1) as f64;
    let scale = (f64::from(area.width) * cw / f64::from(pixels.width))
        .min(f64::from(area.height) * ch / f64::from(pixels.height));
    let width = ((f64::from(pixels.width) * scale / cw).floor() as u16)
        .max(1)
        .min(area.width);
    let height = ((f64::from(pixels.height) * scale / ch).floor() as u16)
        .max(1)
        .min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
