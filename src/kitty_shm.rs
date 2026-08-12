//! Kitty graphics via POSIX shared memory (`t=s`): the escape sequence sent
//! through the pty carries only a base64 shm object *name*; the terminal
//! mmaps, reads, and unlinks the object itself. This replaces the
//! `ratatui-image` `t=d` path (full base64 RGBA per frame through the pty)
//! for local kitty-protocol terminals, cutting per-frame pty traffic from
//! megabytes to ~100 bytes. Placement uses the same Unicode-placeholder
//! (`U=1`) scheme as `ratatui-image`, so it composes with ratatui's cell
//! diffing. See docs/superpowers/specs/2026-08-12-kitty-shm-transfer-design.md.

use std::cell::Cell;
use std::ffi::CString;
use std::fmt::Write as _;
use std::num::NonZeroU16;

use anyhow::{Context, Result, anyhow};
use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;

use crate::video::Frame;

/// The single image id every frame is transmitted under: kitty replaces the
/// image data atomically when an existing id is retransmitted, and every
/// placement referencing it updates. One id means the placeholder cells
/// never change between frames (no per-frame repaint, no delete gap — the
/// flicker sources), and no delete escape is ever needed mid-stream.
/// Value is arbitrary but fixed; `ratatui-image` uses random ids, so a
/// collision with a concurrent crate-path image is effectively impossible.
const IMAGE_ID: u32 = 0x00A0_A001;

/// Pure function: should the shm transport be used? Kitty protocol only, and
/// only when the terminal is on this machine (shm cannot cross an SSH
/// connection) and not behind tmux (escape passthrough + locality issues).
pub fn shm_supported(kitty: bool, ssh: bool, tmux: bool) -> bool {
    kitty && !ssh && !tmux
}

/// Kitty shared-memory frame transport for one app session.
pub struct ShmKittyTransport {
    /// Frame counter; selects the shm name slot.
    seq: u64,
    /// Transmit escape for the newest frame, to prepend to the first
    /// placeholder row of the next render. `Cell` because rendering
    /// happens through `&self` (ratatui widgets render from `&App`).
    pending: Cell<Option<String>>,
}

impl ShmKittyTransport {
    pub fn new() -> Self {
        Self {
            seq: 0,
            pending: Cell::new(None),
        }
    }

    /// Publish a decoded frame: copy it into a fresh shm object and queue
    /// the transmit escape for the next render. `cols` x `rows` is the pane
    /// the terminal scales the image onto (the `c`/`r` placement grid).
    pub fn push_frame(&mut self, frame: &Frame, cols: u16, rows: u16) -> Result<()> {
        self.seq += 1;
        let name = shm_name(self.seq);
        shm_write(&name, &frame.rgb)?;

        let escapes = transmit_escape(
            &name,
            frame.width as u32,
            frame.height_px as u32,
            cols,
            rows,
        );
        self.pending.set(Some(escapes));
        Ok(())
    }

    /// Whether a frame has been pushed since construction.
    pub fn has_frame(&self) -> bool {
        self.seq > 0
    }

    /// Draw the Unicode-placeholder grid over `area`, emitting any pending
    /// transmit escapes with the first row. The terminal scales the image to
    /// the placeholder grid on the GPU, so the grid always spans the whole
    /// pane. Mirrors `ratatui-image`'s kitty rendering.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || self.seq == 0 {
            return;
        }
        let [id_extra, id_r, id_g, id_b] = IMAGE_ID.to_be_bytes();
        let id_color = format!("\x1b[38;2;{id_r};{id_g};{id_b}m");

        let width = usize::from(area.width);
        let row_tail: String = std::iter::repeat_n('\u{10EEEE}', width - 1).collect();
        // Restore the saved cursor, then step to the end of the row so the
        // terminal's cursor bookkeeping matches the cell the buffer thinks
        // was written last.
        let right = area.width - 1;
        let restore_cursor = format!("\x1b[u\x1b[{right}C");

        let mut pending = self.pending.take();
        // The placeholder row/column diacritic table has 297 entries; panes
        // are far shorter in practice.
        let height = area.height.min(DIACRITICS.len() as u16);
        for y in 0..height {
            let mut symbol = String::new();
            if let Some(escapes) = pending.take() {
                symbol.push_str(&escapes);
            }
            // Save cursor, set fg color to the image id, then one
            // fully-specified placeholder (row, column 0, id high byte);
            // the rest of the row inherits its diacritics.
            write!(
                symbol,
                "\x1b[s{id_color}\u{10EEEE}{}{}{}",
                DIACRITICS[y as usize], DIACRITICS[0], DIACRITICS[id_extra as usize],
            )
            .unwrap();
            symbol.push_str(&row_tail);
            symbol.push_str(&restore_cursor);

            // The whole row lives in the first cell's symbol; the remaining
            // cells are skipped so the diff never overwrites the row.
            for x in 1..area.width {
                if let Some(cell) = buf.cell_mut((area.left() + x, area.top() + y)) {
                    cell.set_diff_option(CellDiffOption::Skip);
                }
            }
            if let Some(cell) = buf.cell_mut((area.left(), area.top() + y)) {
                cell.set_symbol(&symbol)
                    .set_diff_option(CellDiffOption::ForcedWidth(NonZeroU16::new(1).unwrap()));
            }
        }
    }
}

/// Shm object name for a frame. Bounded rotation (8 slots) so a terminal
/// that stops reading can strand at most 8 objects; each write unlinks the
/// slot first. Kept short: macOS caps shm names at 31 characters.
fn shm_name(seq: u64) -> String {
    format!("/yt{:04x}.{}", std::process::id() & 0xFFFF, seq % 8)
}

/// Pure function: kitty transmit-and-virtual-place escape for an shm object
/// holding `w` x `h` RGB24 pixels, scaled onto a `cols` x `rows` cell grid
/// (without `c`/`r` kitty places the image at native pixel size instead of
/// scaling it to the placeholder grid). `q=2` suppresses all terminal
/// responses — this program must never read stdin.
fn transmit_escape(shm_name: &str, w: u32, h: u32, cols: u16, rows: u16) -> String {
    let name_b64 = base64_simd::STANDARD.encode_to_string(shm_name.as_bytes());
    format!(
        "\x1b_Gq=2,i={IMAGE_ID},a=T,U=1,f=24,t=s,s={w},v={h},c={cols},r={rows};{name_b64}\x1b\\"
    )
}

/// Copy `data` into a fresh shm object named `name`. The terminal unlinks
/// the object after reading it (verified in Ghostty's readSharedMemory).
fn shm_write(name: &str, data: &[u8]) -> Result<()> {
    let c_name = CString::new(name).context("shm name contains NUL")?;

    // A previous run (or an unread slot) may have left the name behind;
    // O_EXCL below would then fail forever.
    // SAFETY: c_name is a valid NUL-terminated string.
    unsafe { libc::shm_unlink(c_name.as_ptr()) };

    // SAFETY: c_name is a valid NUL-terminated string; flags/mode are plain
    // integer constants.
    let fd = unsafe {
        libc::shm_open(
            c_name.as_ptr(),
            libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
            0o600,
        )
    };
    if fd < 0 {
        return Err(anyhow!(
            "Failed to create shm object: {}",
            std::io::Error::last_os_error()
        ));
    }

    let result = write_via_mmap(fd, data);

    // SAFETY: fd is a valid descriptor owned by this function.
    unsafe { libc::close(fd) };
    result
}

fn write_via_mmap(fd: i32, data: &[u8]) -> Result<()> {
    let len = data.len();
    // SAFETY: fd is a valid open shm descriptor.
    if unsafe { libc::ftruncate(fd, len as libc::off_t) } != 0 {
        return Err(anyhow!(
            "Failed to size shm object: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: fd is valid and has just been sized to `len` bytes.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(anyhow!(
            "Failed to map shm object: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: ptr maps exactly `len` writable bytes and data is `len` long;
    // the regions cannot overlap (one is a fresh shm mapping).
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, len) };
    // SAFETY: ptr is a live mapping of exactly `len` bytes, unmapped once.
    unsafe { libc::munmap(ptr, len) };
    Ok(())
}

/// Kitty row/column diacritics, from
/// <https://sw.kovidgoyal.net/kitty/_downloads/1792bad15b12979994cd6ecc54c967a6/rowcolumn-diacritics.txt>
/// (same table `ratatui-image` embeds).
static DIACRITICS: [char; 297] = [
    '\u{305}',
    '\u{30D}',
    '\u{30E}',
    '\u{310}',
    '\u{312}',
    '\u{33D}',
    '\u{33E}',
    '\u{33F}',
    '\u{346}',
    '\u{34A}',
    '\u{34B}',
    '\u{34C}',
    '\u{350}',
    '\u{351}',
    '\u{352}',
    '\u{357}',
    '\u{35B}',
    '\u{363}',
    '\u{364}',
    '\u{365}',
    '\u{366}',
    '\u{367}',
    '\u{368}',
    '\u{369}',
    '\u{36A}',
    '\u{36B}',
    '\u{36C}',
    '\u{36D}',
    '\u{36E}',
    '\u{36F}',
    '\u{483}',
    '\u{484}',
    '\u{485}',
    '\u{486}',
    '\u{487}',
    '\u{592}',
    '\u{593}',
    '\u{594}',
    '\u{595}',
    '\u{597}',
    '\u{598}',
    '\u{599}',
    '\u{59C}',
    '\u{59D}',
    '\u{59E}',
    '\u{59F}',
    '\u{5A0}',
    '\u{5A1}',
    '\u{5A8}',
    '\u{5A9}',
    '\u{5AB}',
    '\u{5AC}',
    '\u{5AF}',
    '\u{5C4}',
    '\u{610}',
    '\u{611}',
    '\u{612}',
    '\u{613}',
    '\u{614}',
    '\u{615}',
    '\u{616}',
    '\u{617}',
    '\u{657}',
    '\u{658}',
    '\u{659}',
    '\u{65A}',
    '\u{65B}',
    '\u{65D}',
    '\u{65E}',
    '\u{6D6}',
    '\u{6D7}',
    '\u{6D8}',
    '\u{6D9}',
    '\u{6DA}',
    '\u{6DB}',
    '\u{6DC}',
    '\u{6DF}',
    '\u{6E0}',
    '\u{6E1}',
    '\u{6E2}',
    '\u{6E4}',
    '\u{6E7}',
    '\u{6E8}',
    '\u{6EB}',
    '\u{6EC}',
    '\u{730}',
    '\u{732}',
    '\u{733}',
    '\u{735}',
    '\u{736}',
    '\u{73A}',
    '\u{73D}',
    '\u{73F}',
    '\u{740}',
    '\u{741}',
    '\u{743}',
    '\u{745}',
    '\u{747}',
    '\u{749}',
    '\u{74A}',
    '\u{7EB}',
    '\u{7EC}',
    '\u{7ED}',
    '\u{7EE}',
    '\u{7EF}',
    '\u{7F0}',
    '\u{7F1}',
    '\u{7F3}',
    '\u{816}',
    '\u{817}',
    '\u{818}',
    '\u{819}',
    '\u{81B}',
    '\u{81C}',
    '\u{81D}',
    '\u{81E}',
    '\u{81F}',
    '\u{820}',
    '\u{821}',
    '\u{822}',
    '\u{823}',
    '\u{825}',
    '\u{826}',
    '\u{827}',
    '\u{829}',
    '\u{82A}',
    '\u{82B}',
    '\u{82C}',
    '\u{82D}',
    '\u{951}',
    '\u{953}',
    '\u{954}',
    '\u{F82}',
    '\u{F83}',
    '\u{F86}',
    '\u{F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shm_supported_requires_local_kitty() {
        assert!(shm_supported(true, false, false));
        assert!(!shm_supported(true, true, false)); // ssh
        assert!(!shm_supported(true, false, true)); // tmux
        assert!(!shm_supported(false, false, false)); // iterm2/sixel
    }

    #[test]
    fn transmit_escape_matches_kitty_protocol_shape() {
        let escape = transmit_escape("/yt1234.0", 640, 360, 120, 30);
        assert_eq!(
            escape,
            "\x1b_Gq=2,i=10526721,a=T,U=1,f=24,t=s,s=640,v=360,c=120,r=30;L3l0MTIzNC4w\x1b\\"
        );
    }

    #[test]
    fn shm_names_stay_within_macos_31_char_limit_and_rotate() {
        let name = shm_name(1);
        assert!(name.len() <= 31);
        assert!(name.starts_with("/yt"));
        assert_eq!(shm_name(1), shm_name(9)); // 8-slot rotation
        assert_ne!(shm_name(1), shm_name(2));
    }

    #[test]
    fn shm_write_roundtrips_through_the_object() {
        let name = "/yt-test-roundtrip";
        let data = vec![7u8; 1024];
        shm_write(name, &data).unwrap();

        // Read it back through a second shm_open + mmap (macOS shm fds do
        // not support read(2)) to prove the bytes landed.
        let c_name = CString::new(name).unwrap();
        // SAFETY: valid name; read-only open of the object just created.
        let fd = unsafe { libc::shm_open(c_name.as_ptr(), libc::O_RDONLY, 0) };
        assert!(fd >= 0);
        // SAFETY: fd is a valid shm descriptor holding 1024 bytes.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                1024,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        assert_ne!(ptr, libc::MAP_FAILED);
        // SAFETY: ptr maps exactly 1024 readable bytes.
        let back = unsafe { std::slice::from_raw_parts(ptr as *const u8, 1024).to_vec() };
        // SAFETY: ptr/fd owned here; name no longer needed.
        unsafe {
            libc::munmap(ptr, 1024);
            libc::close(fd);
            libc::shm_unlink(c_name.as_ptr());
        }
        assert_eq!(back, data);
    }

    #[test]
    fn push_frame_queues_a_transmit_escape_with_the_pane_grid() {
        let mut transport = ShmKittyTransport::new();
        let frame = Frame {
            width: 2,
            height_px: 2,
            rgb: vec![0u8; 12],
        };
        transport.push_frame(&frame, 80, 24).unwrap();

        let pending = transport.pending.take().unwrap();
        assert!(pending.contains("a=T,U=1,f=24,t=s,s=2,v=2,c=80,r=24"));
        assert!(transport.has_frame());

        // Clean up the shm slot the test created.
        let c_name = CString::new(shm_name(1)).unwrap();
        // SAFETY: valid NUL-terminated name.
        unsafe { libc::shm_unlink(c_name.as_ptr()) };
    }

    #[test]
    fn render_paints_placeholders_with_pending_escapes_in_first_row() {
        let mut transport = ShmKittyTransport::new();
        transport.seq = 1;
        transport.pending.set(Some("ESCAPES".to_string()));

        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);
        transport.render(area, &mut buf);

        let first = buf.cell((0, 0)).unwrap().symbol();
        assert!(first.starts_with("ESCAPES"));
        assert!(first.contains('\u{10EEEE}'));
        let second_row = buf.cell((0, 1)).unwrap().symbol();
        assert!(!second_row.contains("ESCAPES"));
        assert!(second_row.contains('\u{10EEEE}'));
    }
}
