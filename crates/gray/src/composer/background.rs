//! Host-owned Kitty placement. The plugin supplies an already-prepared PNG.
use std::io::{Read, Write};
use std::path::Path;

use base64::Engine;

pub(crate) struct Background {
    png: String,
    id: u32,
    uploaded: bool,
}

impl Background {
    pub(crate) fn load(path: &Path) -> anyhow::Result<Self> {
        anyhow::ensure!(path.is_absolute(), "background path must be absolute");
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        anyhow::ensure!(
            file.metadata()?.is_file(),
            "background must be a regular file"
        );
        let mut bytes = Vec::new();
        file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= 8 * 1024 * 1024, "PNG exceeds 8 MiB");
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(&bytes), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        reader.decode()?;
        // Two adjacent IDs: wallpaper and transparent layering sentinel.
        let id = ((uuid::Uuid::new_v4().as_u128() as u32) & 0x7fff_fffe) + 1;
        Ok(Self {
            png: base64::engine::general_purpose::STANDARD.encode(bytes),
            id,
            uploaded: false,
        })
    }

    pub(crate) fn draw(
        &mut self,
        out: &mut impl Write,
        cols: u16,
        rows: u16,
    ) -> std::io::Result<()> {
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        if !self.uploaded {
            // Mark before writing so a partial upload is cleaned up too.
            self.uploaded = true;
            for (index, chunk) in self.png.as_bytes().chunks(4096).enumerate() {
                let more = usize::from((index + 1) * 4096 < self.png.len());
                if index == 0 {
                    write!(out, "\x1b_Ga=t,f=100,i={},q=2,m={more};", self.id)?;
                } else {
                    write!(out, "\x1b_Gm={more};")?;
                }
                out.write_all(chunk)?;
                out.write_all(b"\x1b\\")?;
            }
        }
        write!(out, "\x1b7\x1b[H")?;
        // Ghostty 1.1.3 partitions negative-z-only images incorrectly. A transparent
        // nonnegative placement supplies the boundary; no visible overlay or timer.
        write!(
            out,
            "\x1b_Ga=T,f=32,s=1,v=1,i={},p=1,z=0,C=1,q=2;AAAAAA==\x1b\\",
            self.id + 1
        )?;
        write!(
            out,
            "\x1b_Ga=p,i={},p=1,c={cols},r={rows},z=-1,C=1,q=2\x1b\\\x1b8",
            self.id
        )?;
        out.flush()
    }

    pub(crate) fn hide(&mut self, out: &mut impl Write) -> std::io::Result<()> {
        if self.uploaded {
            for id in [self.id, self.id + 1] {
                write!(out, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")?;
            }
            out.flush()?;
            self.uploaded = false;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "background_tests.rs"]
mod tests;
