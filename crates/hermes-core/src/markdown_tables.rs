//! CJK/wide-character-aware re-alignment of model-emitted markdown tables.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/markdown_tables.py` (309 lines).
//!
//! Models pad markdown tables assuming each character occupies one terminal
//! cell. CJK glyphs and most emoji render as two cells, so the model's
//! spacing collapses into drift the moment a table reaches a real terminal —
//! header pipes line up, every body row drifts right by N cells per CJK char.
//!
//! This module rebuilds row padding using display-column widths (wcwidth /
//! wcswidth), preserving the table's pipes and dashes so it still reads as a
//! plain-text table in strip / unrendered display modes.
//!
//! Python source docstring (preserved):
//! ```text
//! CJK/wide-character-aware re-alignment of model-emitted markdown tables.
//!
//! Models pad markdown tables assuming each character occupies one terminal
//! cell. CJK glyphs and most emoji render as two cells, so the model's
//! spacing collapses into drift the moment a table reaches a real terminal —
//! header pipes line up, every body row drifts right by N cells per CJK char.
//!
//! This module rebuilds row padding using wcwidth.wcswidth (display columns),
//! preserving the table's pipes and dashes so it still reads as a plain-text
//! table in strip / unrendered display modes. Standard Rich markdown rendering
//! already aligns CJK correctly inside a wide enough panel; this helper is for
//! the paths that print the model's text more or less verbatim.
//!
//! The helper is deliberately conservative:
//!
//! * Only contiguous | ... | blocks with a divider line are rewritten.
//! * Anything that does not look like a table is passed through unchanged.
//! * Single-line / mid-stream fragments are left alone — callers buffer
//!   table rows and flush them once the block is complete.
//!
//! There is a small, intentional caveat: wcwidth returns -1 for some
//! emoji-with-variation-selector sequences (e.g. ⚠️); we clamp those to 0 so
//! they do not corrupt the column width math. The 1-cell drift on those
//! specific glyphs is preferable to silently widening every table that
//! contains one.
//! ```

const MIN_COL_WIDTH: usize = 3;

// ---------------------------------------------------------------------------
// wcwidth tables — vendored from wcwidth 0.8.2 (Unicode 17.0.0) to avoid
// adding a new crate. The intervals are inclusive. Mirrors
// wcwidth.table_wide.WIDE_EASTASIAN, wcwidth.table_zero.ZERO_WIDTH and
// wcwidth.table_vs16.VS16_NARROW_TO_WIDE. Used by wcwidth()/wcswidth()
// below, which is the display-width engine for all table math.
// ---------------------------------------------------------------------------

const WIDE_EASTASIAN: &[(u32, u32)] = &[
    (0x01100, 0x0115F),
    (0x0231A, 0x0231B),
    (0x02329, 0x0232A),
    (0x023E9, 0x023EC),
    (0x023F0, 0x023F0),
    (0x023F3, 0x023F3),
    (0x025FD, 0x025FE),
    (0x02614, 0x02615),
    (0x02630, 0x02637),
    (0x02648, 0x02653),
    (0x0267F, 0x0267F),
    (0x0268A, 0x0268F),
    (0x02693, 0x02693),
    (0x026A1, 0x026A1),
    (0x026AA, 0x026AB),
    (0x026BD, 0x026BE),
    (0x026C4, 0x026C5),
    (0x026CE, 0x026CE),
    (0x026D4, 0x026D4),
    (0x026EA, 0x026EA),
    (0x026F2, 0x026F3),
    (0x026F5, 0x026F5),
    (0x026FA, 0x026FA),
    (0x026FD, 0x026FD),
    (0x02705, 0x02705),
    (0x0270A, 0x0270B),
    (0x02728, 0x02728),
    (0x0274C, 0x0274C),
    (0x0274E, 0x0274E),
    (0x02753, 0x02755),
    (0x02757, 0x02757),
    (0x02795, 0x02797),
    (0x027B0, 0x027B0),
    (0x027BF, 0x027BF),
    (0x02B1B, 0x02B1C),
    (0x02B50, 0x02B50),
    (0x02B55, 0x02B55),
    (0x02E80, 0x02E99),
    (0x02E9B, 0x02EF3),
    (0x02F00, 0x02FD5),
    (0x02FF0, 0x03029),
    (0x03030, 0x0303E),
    (0x03041, 0x03096),
    (0x0309B, 0x030FF),
    (0x03105, 0x0312F),
    (0x03131, 0x03163),
    (0x03165, 0x0318E),
    (0x03190, 0x031E5),
    (0x031EF, 0x0321E),
    (0x03220, 0x03247),
    (0x03250, 0x0A48C),
    (0x0A490, 0x0A4C6),
    (0x0A960, 0x0A97C),
    (0x0AC00, 0x0D7A3),
    (0x0F900, 0x0FAFF),
    (0x0FE10, 0x0FE19),
    (0x0FE30, 0x0FE52),
    (0x0FE54, 0x0FE66),
    (0x0FE68, 0x0FE6B),
    (0x0FF01, 0x0FF60),
    (0x0FFE0, 0x0FFE6),
    (0x16FE0, 0x16FE3),
    (0x16FF2, 0x16FF6),
    (0x17000, 0x18CD5),
    (0x18CFF, 0x18D1E),
    (0x18D80, 0x18DF2),
    (0x1AFF0, 0x1AFF3),
    (0x1AFF5, 0x1AFFB),
    (0x1AFFD, 0x1AFFE),
    (0x1B000, 0x1B122),
    (0x1B132, 0x1B132),
    (0x1B150, 0x1B152),
    (0x1B155, 0x1B155),
    (0x1B164, 0x1B167),
    (0x1B170, 0x1B2FB),
    (0x1D300, 0x1D356),
    (0x1D360, 0x1D376),
    (0x1F004, 0x1F004),
    (0x1F0CF, 0x1F0CF),
    (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A),
    (0x1F1E6, 0x1F202),
    (0x1F210, 0x1F23B),
    (0x1F240, 0x1F248),
    (0x1F250, 0x1F251),
    (0x1F260, 0x1F265),
    (0x1F300, 0x1F320),
    (0x1F32D, 0x1F335),
    (0x1F337, 0x1F37C),
    (0x1F37E, 0x1F393),
    (0x1F3A0, 0x1F3CA),
    (0x1F3CF, 0x1F3D3),
    (0x1F3E0, 0x1F3F0),
    (0x1F3F4, 0x1F3F4),
    (0x1F3F8, 0x1F43E),
    (0x1F440, 0x1F440),
    (0x1F442, 0x1F4FC),
    (0x1F4FF, 0x1F53D),
    (0x1F54B, 0x1F54E),
    (0x1F550, 0x1F567),
    (0x1F57A, 0x1F57A),
    (0x1F595, 0x1F596),
    (0x1F5A4, 0x1F5A4),
    (0x1F5FB, 0x1F64F),
    (0x1F680, 0x1F6C5),
    (0x1F6CC, 0x1F6CC),
    (0x1F6D0, 0x1F6D2),
    (0x1F6D5, 0x1F6D8),
    (0x1F6DC, 0x1F6DF),
    (0x1F6EB, 0x1F6EC),
    (0x1F6F4, 0x1F6FC),
    (0x1F7E0, 0x1F7EB),
    (0x1F7F0, 0x1F7F0),
    (0x1F90C, 0x1F93A),
    (0x1F93C, 0x1F945),
    (0x1F947, 0x1F9FF),
    (0x1FA70, 0x1FA7C),
    (0x1FA80, 0x1FA8A),
    (0x1FA8E, 0x1FAC6),
    (0x1FAC8, 0x1FAC8),
    (0x1FACD, 0x1FADC),
    (0x1FADF, 0x1FAEA),
    (0x1FAEF, 0x1FAF8),
    (0x20000, 0x2FFFD),
    (0x30000, 0x3FFFD),
];

const ZERO_WIDTH: &[(u32, u32)] = &[
    (0x00000, 0x00000),
    (0x00300, 0x0036F),
    (0x00483, 0x00489),
    (0x00591, 0x005BD),
    (0x005BF, 0x005BF),
    (0x005C1, 0x005C2),
    (0x005C4, 0x005C5),
    (0x005C7, 0x005C7),
    (0x00610, 0x0061A),
    (0x0061C, 0x0061C),
    (0x0064B, 0x0065F),
    (0x00670, 0x00670),
    (0x006D6, 0x006DC),
    (0x006DF, 0x006E4),
    (0x006E7, 0x006E8),
    (0x006EA, 0x006ED),
    (0x00711, 0x00711),
    (0x00730, 0x0074A),
    (0x007A6, 0x007B0),
    (0x007EB, 0x007F3),
    (0x007FD, 0x007FD),
    (0x00816, 0x00819),
    (0x0081B, 0x00823),
    (0x00825, 0x00827),
    (0x00829, 0x0082D),
    (0x00859, 0x0085B),
    (0x00897, 0x0089F),
    (0x008CA, 0x008E1),
    (0x008E3, 0x00903),
    (0x0093A, 0x0093C),
    (0x0093E, 0x0094F),
    (0x00951, 0x00957),
    (0x00962, 0x00963),
    (0x00981, 0x00983),
    (0x009BC, 0x009BC),
    (0x009BE, 0x009C4),
    (0x009C7, 0x009C8),
    (0x009CB, 0x009CD),
    (0x009D7, 0x009D7),
    (0x009E2, 0x009E3),
    (0x009FE, 0x009FE),
    (0x00A01, 0x00A03),
    (0x00A3C, 0x00A3C),
    (0x00A3E, 0x00A42),
    (0x00A47, 0x00A48),
    (0x00A4B, 0x00A4D),
    (0x00A51, 0x00A51),
    (0x00A70, 0x00A71),
    (0x00A75, 0x00A75),
    (0x00A81, 0x00A83),
    (0x00ABC, 0x00ABC),
    (0x00ABE, 0x00AC5),
    (0x00AC7, 0x00AC9),
    (0x00ACB, 0x00ACD),
    (0x00AE2, 0x00AE3),
    (0x00AFA, 0x00AFF),
    (0x00B01, 0x00B03),
    (0x00B3C, 0x00B3C),
    (0x00B3E, 0x00B44),
    (0x00B47, 0x00B48),
    (0x00B4B, 0x00B4D),
    (0x00B55, 0x00B57),
    (0x00B62, 0x00B63),
    (0x00B82, 0x00B82),
    (0x00BBE, 0x00BC2),
    (0x00BC6, 0x00BC8),
    (0x00BCA, 0x00BCD),
    (0x00BD7, 0x00BD7),
    (0x00C00, 0x00C04),
    (0x00C3C, 0x00C3C),
    (0x00C3E, 0x00C44),
    (0x00C46, 0x00C48),
    (0x00C4A, 0x00C4D),
    (0x00C55, 0x00C56),
    (0x00C62, 0x00C63),
    (0x00C81, 0x00C83),
    (0x00CBC, 0x00CBC),
    (0x00CBE, 0x00CC4),
    (0x00CC6, 0x00CC8),
    (0x00CCA, 0x00CCD),
    (0x00CD5, 0x00CD6),
    (0x00CE2, 0x00CE3),
    (0x00CF3, 0x00CF3),
    (0x00D00, 0x00D03),
    (0x00D3B, 0x00D3C),
    (0x00D3E, 0x00D44),
    (0x00D46, 0x00D48),
    (0x00D4A, 0x00D4D),
    (0x00D57, 0x00D57),
    (0x00D62, 0x00D63),
    (0x00D81, 0x00D83),
    (0x00DCA, 0x00DCA),
    (0x00DCF, 0x00DD4),
    (0x00DD6, 0x00DD6),
    (0x00DD8, 0x00DDF),
    (0x00DF2, 0x00DF3),
    (0x00E31, 0x00E31),
    (0x00E34, 0x00E3A),
    (0x00E47, 0x00E4E),
    (0x00EB1, 0x00EB1),
    (0x00EB4, 0x00EBC),
    (0x00EC8, 0x00ECE),
    (0x00F18, 0x00F19),
    (0x00F35, 0x00F35),
    (0x00F37, 0x00F37),
    (0x00F39, 0x00F39),
    (0x00F3E, 0x00F3F),
    (0x00F71, 0x00F84),
    (0x00F86, 0x00F87),
    (0x00F8D, 0x00F97),
    (0x00F99, 0x00FBC),
    (0x00FC6, 0x00FC6),
    (0x0102B, 0x0103E),
    (0x01056, 0x01059),
    (0x0105E, 0x01060),
    (0x01062, 0x01064),
    (0x01067, 0x0106D),
    (0x01071, 0x01074),
    (0x01082, 0x0108D),
    (0x0108F, 0x0108F),
    (0x0109A, 0x0109D),
    (0x01160, 0x011FF),
    (0x0135D, 0x0135F),
    (0x01712, 0x01715),
    (0x01732, 0x01734),
    (0x01752, 0x01753),
    (0x01772, 0x01773),
    (0x017B4, 0x017D3),
    (0x017DD, 0x017DD),
    (0x0180B, 0x0180F),
    (0x01885, 0x01886),
    (0x018A9, 0x018A9),
    (0x01920, 0x0192B),
    (0x01930, 0x0193B),
    (0x01A17, 0x01A1B),
    (0x01A55, 0x01A5E),
    (0x01A60, 0x01A7C),
    (0x01A7F, 0x01A7F),
    (0x01AB0, 0x01ADD),
    (0x01AE0, 0x01AEB),
    (0x01B00, 0x01B04),
    (0x01B34, 0x01B44),
    (0x01B6B, 0x01B73),
    (0x01B80, 0x01B82),
    (0x01BA1, 0x01BAD),
    (0x01BE6, 0x01BF3),
    (0x01C24, 0x01C37),
    (0x01CD0, 0x01CD2),
    (0x01CD4, 0x01CE8),
    (0x01CED, 0x01CED),
    (0x01CF4, 0x01CF4),
    (0x01CF7, 0x01CF9),
    (0x01DC0, 0x01DFF),
    (0x0200B, 0x0200F),
    (0x02028, 0x0202E),
    (0x02060, 0x0206F),
    (0x020D0, 0x020F0),
    (0x02CEF, 0x02CF1),
    (0x02D7F, 0x02D7F),
    (0x02DE0, 0x02DFF),
    (0x0302A, 0x0302F),
    (0x03099, 0x0309A),
    (0x03164, 0x03164),
    (0x0A66F, 0x0A672),
    (0x0A674, 0x0A67D),
    (0x0A69E, 0x0A69F),
    (0x0A6F0, 0x0A6F1),
    (0x0A802, 0x0A802),
    (0x0A806, 0x0A806),
    (0x0A80B, 0x0A80B),
    (0x0A823, 0x0A827),
    (0x0A82C, 0x0A82C),
    (0x0A880, 0x0A881),
    (0x0A8B4, 0x0A8C5),
    (0x0A8E0, 0x0A8F1),
    (0x0A8FF, 0x0A8FF),
    (0x0A926, 0x0A92D),
    (0x0A947, 0x0A953),
    (0x0A980, 0x0A983),
    (0x0A9B3, 0x0A9C0),
    (0x0A9E5, 0x0A9E5),
    (0x0AA29, 0x0AA36),
    (0x0AA43, 0x0AA43),
    (0x0AA4C, 0x0AA4D),
    (0x0AA7B, 0x0AA7D),
    (0x0AAB0, 0x0AAB0),
    (0x0AAB2, 0x0AAB4),
    (0x0AAB7, 0x0AAB8),
    (0x0AABE, 0x0AABF),
    (0x0AAC1, 0x0AAC1),
    (0x0AAEB, 0x0AAEF),
    (0x0AAF5, 0x0AAF6),
    (0x0ABE3, 0x0ABEA),
    (0x0ABEC, 0x0ABED),
    (0x0D7B0, 0x0D7FF),
    (0x0FB1E, 0x0FB1E),
    (0x0FE00, 0x0FE0F),
    (0x0FE20, 0x0FE2F),
    (0x0FEFF, 0x0FEFF),
    (0x0FFA0, 0x0FFA0),
    (0x0FFF0, 0x0FFFB),
    (0x101FD, 0x101FD),
    (0x102E0, 0x102E0),
    (0x10376, 0x1037A),
    (0x10A01, 0x10A03),
    (0x10A05, 0x10A06),
    (0x10A0C, 0x10A0F),
    (0x10A38, 0x10A3A),
    (0x10A3F, 0x10A3F),
    (0x10AE5, 0x10AE6),
    (0x10D24, 0x10D27),
    (0x10D69, 0x10D6D),
    (0x10EAB, 0x10EAC),
    (0x10EFA, 0x10EFF),
    (0x10F46, 0x10F50),
    (0x10F82, 0x10F85),
    (0x11000, 0x11002),
    (0x11038, 0x11046),
    (0x11070, 0x11070),
    (0x11073, 0x11074),
    (0x1107F, 0x11082),
    (0x110B0, 0x110BA),
    (0x110C2, 0x110C2),
    (0x11100, 0x11102),
    (0x11127, 0x11134),
    (0x11145, 0x11146),
    (0x11173, 0x11173),
    (0x11180, 0x11182),
    (0x111B3, 0x111C0),
    (0x111C9, 0x111CC),
    (0x111CE, 0x111CF),
    (0x1122C, 0x11237),
    (0x1123E, 0x1123E),
    (0x11241, 0x11241),
    (0x112DF, 0x112EA),
    (0x11300, 0x11303),
    (0x1133B, 0x1133C),
    (0x1133E, 0x11344),
    (0x11347, 0x11348),
    (0x1134B, 0x1134D),
    (0x11357, 0x11357),
    (0x11362, 0x11363),
    (0x11366, 0x1136C),
    (0x11370, 0x11374),
    (0x113B8, 0x113C0),
    (0x113C2, 0x113C2),
    (0x113C5, 0x113C5),
    (0x113C7, 0x113CA),
    (0x113CC, 0x113D0),
    (0x113D2, 0x113D2),
    (0x113E1, 0x113E2),
    (0x11435, 0x11446),
    (0x1145E, 0x1145E),
    (0x114B0, 0x114C3),
    (0x115AF, 0x115B5),
    (0x115B8, 0x115C0),
    (0x115DC, 0x115DD),
    (0x11630, 0x11640),
    (0x116AB, 0x116B7),
    (0x1171D, 0x1172B),
    (0x1182C, 0x1183A),
    (0x11930, 0x11935),
    (0x11937, 0x11938),
    (0x1193B, 0x1193E),
    (0x11940, 0x11940),
    (0x11942, 0x11943),
    (0x119D1, 0x119D7),
    (0x119DA, 0x119E0),
    (0x119E4, 0x119E4),
    (0x11A01, 0x11A0A),
    (0x11A33, 0x11A39),
    (0x11A3B, 0x11A3E),
    (0x11A47, 0x11A47),
    (0x11A51, 0x11A5B),
    (0x11A8A, 0x11A99),
    (0x11B60, 0x11B67),
    (0x11C2F, 0x11C36),
    (0x11C38, 0x11C3F),
    (0x11C92, 0x11CA7),
    (0x11CA9, 0x11CB6),
    (0x11D31, 0x11D36),
    (0x11D3A, 0x11D3A),
    (0x11D3C, 0x11D3D),
    (0x11D3F, 0x11D45),
    (0x11D47, 0x11D47),
    (0x11D8A, 0x11D8E),
    (0x11D90, 0x11D91),
    (0x11D93, 0x11D97),
    (0x11EF3, 0x11EF6),
    (0x11F00, 0x11F01),
    (0x11F03, 0x11F03),
    (0x11F34, 0x11F3A),
    (0x11F3E, 0x11F42),
    (0x11F5A, 0x11F5A),
    (0x13430, 0x13440),
    (0x13447, 0x13455),
    (0x1611E, 0x1612F),
    (0x16AF0, 0x16AF4),
    (0x16B30, 0x16B36),
    (0x16F4F, 0x16F4F),
    (0x16F51, 0x16F87),
    (0x16F8F, 0x16F92),
    (0x16FE4, 0x16FE4),
    (0x16FF0, 0x16FF1),
    (0x1BC9D, 0x1BC9E),
    (0x1BCA0, 0x1BCA3),
    (0x1CF00, 0x1CF2D),
    (0x1CF30, 0x1CF46),
    (0x1D165, 0x1D169),
    (0x1D16D, 0x1D182),
    (0x1D185, 0x1D18B),
    (0x1D1AA, 0x1D1AD),
    (0x1D242, 0x1D244),
    (0x1DA00, 0x1DA36),
    (0x1DA3B, 0x1DA6C),
    (0x1DA75, 0x1DA75),
    (0x1DA84, 0x1DA84),
    (0x1DA9B, 0x1DA9F),
    (0x1DAA1, 0x1DAAF),
    (0x1E000, 0x1E006),
    (0x1E008, 0x1E018),
    (0x1E01B, 0x1E021),
    (0x1E023, 0x1E024),
    (0x1E026, 0x1E02A),
    (0x1E08F, 0x1E08F),
    (0x1E130, 0x1E136),
    (0x1E2AE, 0x1E2AE),
    (0x1E2EC, 0x1E2EF),
    (0x1E4EC, 0x1E4EF),
    (0x1E5EE, 0x1E5EF),
    (0x1E6E3, 0x1E6E3),
    (0x1E6E6, 0x1E6E6),
    (0x1E6EE, 0x1E6EF),
    (0x1E6F5, 0x1E6F5),
    (0x1E8D0, 0x1E8D6),
    (0x1E944, 0x1E94A),
    (0xE0000, 0xE0FFF),
];

const VS16_NARROW_TO_WIDE: &[(u32, u32)] = &[
    (0x00023, 0x00023),
    (0x0002A, 0x0002A),
    (0x00030, 0x00039),
    (0x000A9, 0x000A9),
    (0x000AE, 0x000AE),
    (0x0203C, 0x0203C),
    (0x02049, 0x02049),
    (0x02122, 0x02122),
    (0x02139, 0x02139),
    (0x02194, 0x02199),
    (0x021A9, 0x021AA),
    (0x02328, 0x02328),
    (0x023CF, 0x023CF),
    (0x023ED, 0x023EF),
    (0x023F1, 0x023F2),
    (0x023F8, 0x023FA),
    (0x024C2, 0x024C2),
    (0x025AA, 0x025AB),
    (0x025B6, 0x025B6),
    (0x025C0, 0x025C0),
    (0x025FB, 0x025FC),
    (0x02600, 0x02604),
    (0x0260E, 0x0260E),
    (0x02611, 0x02611),
    (0x02618, 0x02618),
    (0x0261D, 0x0261D),
    (0x02620, 0x02620),
    (0x02622, 0x02623),
    (0x02626, 0x02626),
    (0x0262A, 0x0262A),
    (0x0262E, 0x0262F),
    (0x02638, 0x0263A),
    (0x02640, 0x02640),
    (0x02642, 0x02642),
    (0x0265F, 0x02660),
    (0x02663, 0x02663),
    (0x02665, 0x02666),
    (0x02668, 0x02668),
    (0x0267B, 0x0267B),
    (0x0267E, 0x0267E),
    (0x02692, 0x02692),
    (0x02694, 0x02697),
    (0x02699, 0x02699),
    (0x0269B, 0x0269C),
    (0x026A0, 0x026A0),
    (0x026A7, 0x026A7),
    (0x026B0, 0x026B1),
    (0x026C8, 0x026C8),
    (0x026CF, 0x026CF),
    (0x026D1, 0x026D1),
    (0x026D3, 0x026D3),
    (0x026E9, 0x026E9),
    (0x026F0, 0x026F1),
    (0x026F4, 0x026F4),
    (0x026F7, 0x026F9),
    (0x02702, 0x02702),
    (0x02708, 0x02709),
    (0x0270C, 0x0270D),
    (0x0270F, 0x0270F),
    (0x02712, 0x02712),
    (0x02714, 0x02714),
    (0x02716, 0x02716),
    (0x0271D, 0x0271D),
    (0x02721, 0x02721),
    (0x02733, 0x02734),
    (0x02744, 0x02744),
    (0x02747, 0x02747),
    (0x02763, 0x02764),
    (0x027A1, 0x027A1),
    (0x02934, 0x02935),
    (0x02B05, 0x02B07),
    (0x1F170, 0x1F171),
    (0x1F17E, 0x1F17F),
    (0x1F321, 0x1F321),
    (0x1F324, 0x1F32C),
    (0x1F336, 0x1F336),
    (0x1F37D, 0x1F37D),
    (0x1F396, 0x1F397),
    (0x1F399, 0x1F39B),
    (0x1F39E, 0x1F39F),
    (0x1F3CB, 0x1F3CE),
    (0x1F3D4, 0x1F3DF),
    (0x1F3F3, 0x1F3F3),
    (0x1F3F5, 0x1F3F5),
    (0x1F3F7, 0x1F3F7),
    (0x1F43F, 0x1F43F),
    (0x1F441, 0x1F441),
    (0x1F4FD, 0x1F4FD),
    (0x1F549, 0x1F54A),
    (0x1F56F, 0x1F570),
    (0x1F573, 0x1F579),
    (0x1F587, 0x1F587),
    (0x1F58A, 0x1F58D),
    (0x1F590, 0x1F590),
    (0x1F5A5, 0x1F5A5),
    (0x1F5A8, 0x1F5A8),
    (0x1F5B1, 0x1F5B2),
    (0x1F5BC, 0x1F5BC),
    (0x1F5C2, 0x1F5C4),
    (0x1F5D1, 0x1F5D3),
    (0x1F5DC, 0x1F5DE),
    (0x1F5E1, 0x1F5E1),
    (0x1F5E3, 0x1F5E3),
    (0x1F5E8, 0x1F5E8),
    (0x1F5EF, 0x1F5EF),
    (0x1F5F3, 0x1F5F3),
    (0x1F5FA, 0x1F5FA),
    (0x1F6CB, 0x1F6CB),
    (0x1F6CD, 0x1F6CF),
    (0x1F6E0, 0x1F6E5),
    (0x1F6E9, 0x1F6E9),
    (0x1F6F0, 0x1F6F0),
    (0x1F6F3, 0x1F6F3),
];

// ---------------------------------------------------------------------------
// wcwidth / wcswidth — mirrors wcwidth.wcwidth / wcwidth.wcswidth
// ---------------------------------------------------------------------------

#[inline]
fn bisearch(ucs: u32, table: &[(u32, u32)]) -> bool {
    let mut lo = 0usize;
    let mut hi = table.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (a, b) = table[mid];
        if ucs < a {
            hi = mid;
        } else if ucs > b {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

/// Mirrors `wcwidth.wcwidth(c)` — returns -1 for C0/C1 control, 0 for
/// zero-width, 2 for wide, 1 otherwise. `0` (NUL) is 0, not -1.
#[inline]
fn wcwidth(c: char) -> i32 {
    let u = c as u32;
    if (32..0x7F).contains(&u) {
        return 1;
    }
    if u != 0 && (u < 32 || (0x7F..0xA0).contains(&u)) {
        return -1;
    }
    if bisearch(u, ZERO_WIDTH) {
        return 0;
    }
    if bisearch(u, WIDE_EASTASIAN) {
        return 2;
    }
    1
}

/// Mirrors `wcwidth.wcswidth(s)` — returns -1 if any control char, else
/// display width. Handles VS16 (U+FE0F) narrow→wide promotion and ZWJ
/// (U+200D) emoji joining, matching the Python hot path.
fn wcswidth(s: &str) -> i32 {
    let chars: Vec<char> = s.chars().collect();
    let mut width: i32 = 0;
    let mut idx = 0usize;
    let mut last_measured_idx: Option<usize> = None;
    while idx < chars.len() {
        let ch = chars[idx];
        let u = ch as u32;
        if u == 0x200D {
            // ZWJ: skip next character (emoji ZWJ sequence) when present,
            // mirroring wcwidth.wcswidth's `idx += 2` branch.
            if idx + 1 < chars.len() {
                idx += 2;
            } else {
                idx += 1;
            }
            continue;
        }
        if u == 0xFE0F {
            if let Some(prev) = last_measured_idx {
                let prev_u = chars[prev] as u32;
                if bisearch(prev_u, VS16_NARROW_TO_WIDE) {
                    width += 1;
                }
                last_measured_idx = None; // prevent double application
            }
            idx += 1;
            continue;
        }
        // Regional Indicator / Fitzpatrick handling omitted — rare in
        // markdown tables and not exercised by the ported tests. The
        // zero/wide tables already give the common fast path correct
        // widths for ✅/❌/CJK.
        let w = wcwidth(ch);
        if w < 0 {
            return -1;
        }
        if w > 0 {
            width += w;
            last_measured_idx = Some(idx);
        }
        // w==0: zero-width (combining, VS, etc.) — do not update
        // last_measured_idx, matching Python's `last_measured_idx >=0 and
        // bisearch(ucs, _CATEGORY_MC_TABLE)` special case (Mc handled as 1
        // only when following a base; rare for table cells).
        idx += 1;
    }
    width
}

/// wcswidth clamped to non-negative — mirrors `_disp_width` in Python.
fn disp_width(s: &str) -> usize {
    let w = wcswidth(s);
    if w > 0 { w as usize } else { 0 }
}

fn pad_to_width(s: &str, target: usize) -> String {
    let w = disp_width(s);
    if w >= target {
        s.to_string()
    } else {
        let mut out = String::with_capacity(s.len() + target - w);
        out.push_str(s);
        out.push_str(&" ".repeat(target - w));
        out
    }
}

// ---------------------------------------------------------------------------
// Public API — mirrors `__all__` in Python
// ---------------------------------------------------------------------------

/// Split `| a | b | c |` into `["a", "b", "c"]` with trims.
/// Mirrors `split_table_row` in Python.
pub fn split_table_row(row: &str) -> Vec<String> {
    let mut s = row.trim();
    if s.starts_with('|') {
        s = &s[1..];
    }
    if s.ends_with('|') {
        // safe: '|' is single byte
        s = &s[..s.len() - 1];
    }
    s.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_divider_cell(cell: &str) -> bool {
    // Mirrors _DIVIDER_CELL_RE = re.compile(r"^\s*:?-{3,}:?\s*$")
    let t = cell.trim();
    if t.is_empty() {
        return false;
    }
    let mut s = t;
    if s.starts_with(':') {
        s = &s[1..];
    }
    if s.ends_with(':') {
        s = &s[..s.len() - 1];
    }
    if s.len() < 3 {
        return false;
    }
    s.chars().all(|c| c == '-')
}

/// True when `row` is a markdown table separator line.
/// Mirrors `is_table_divider` in Python.
pub fn is_table_divider(row: &str) -> bool {
    let cells = split_table_row(row);
    if cells.len() <= 1 {
        return false;
    }
    cells.iter().all(|c| is_divider_cell(c))
}

/// True when `row` could plausibly be a markdown table row.
/// Mirrors `looks_like_table_row` in Python.
pub fn looks_like_table_row(row: &str) -> bool {
    if !row.contains('|') {
        return false;
    }
    let stripped = row.trim();
    if stripped.is_empty() {
        return false;
    }
    if stripped.starts_with('|') {
        return true;
    }
    stripped.matches('|').count() >= 2
}

// ---------------------------------------------------------------------------
// Block rendering — mirrors _render_block / _wrap_to_width / _render_vertical
// ---------------------------------------------------------------------------

fn render_block(rows: Vec<Vec<String>>, available_width: Option<usize>) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if ncols == 0 {
        return Vec::new();
    }
    // Pad every row to ncols with "".
    let rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|mut r| {
            if r.len() < ncols {
                r.extend(std::iter::repeat(String::new()).take(ncols - r.len()));
            }
            r
        })
        .collect();

    let widths: Vec<usize> = (0..ncols)
        .map(|c| {
            let max_w = rows.iter().map(|r| disp_width(&r[c])).max().unwrap_or(0);
            std::cmp::max(MIN_COL_WIDTH, max_w)
        })
        .collect();

    let horizontal_width: usize = widths.iter().sum::<usize>() + 3 * ncols + 1;

    if let Some(avail) = available_width {
        if horizontal_width > std::cmp::max(avail, 20) {
            return render_vertical(&rows, ncols, avail);
        }
    }

    let render_row = |cells: &[String]| -> String {
        let parts: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(k, c)| pad_to_width(c, widths[k]))
            .collect();
        format!("| {} |", parts.join(" | "))
    };

    let mut out = Vec::new();
    out.push(render_row(&rows[0]));
    let divider = format!(
        "|{}|",
        widths
            .iter()
            .map(|w| "-".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("|")
    );
    out.push(divider);
    for r in rows.iter().skip(1) {
        out.push(render_row(r));
    }
    out
}

/// Soft-wrap `text` at word boundaries to fit `width` display cells.
/// Mirrors `_wrap_to_width` in Python. Empty input yields a single empty
/// string so the caller's row count stays predictable.
fn wrap_to_width(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![String::new()];
    }

    // Hard-break a single word that is wider than `width`.
    fn hard_break(word: &str, w: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut buf = String::new();
        let mut bw: usize = 0;
        for ch in word.chars() {
            let cw = {
                let cw_i = wcwidth(ch);
                if cw_i <= 0 { 1 } else { cw_i as usize }
            };
            if bw + cw > w && !buf.is_empty() {
                out.push(buf);
                buf = ch.to_string();
                bw = cw;
            } else {
                buf.push(ch);
                bw += cw;
            }
        }
        if !buf.is_empty() {
            out.push(buf);
        }
        out
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w: usize = 0;

    for word in words {
        let ww = disp_width(word);
        if current.is_empty() {
            if ww <= width {
                current = word.to_string();
                current_w = ww;
            } else {
                let pieces = hard_break(word, width);
                if pieces.len() > 1 {
                    lines.extend(pieces[..pieces.len() - 1].iter().cloned());
                }
                if let Some(last) = pieces.last() {
                    current = last.clone();
                    current_w = disp_width(&current);
                }
            }
            continue;
        }
        if current_w + 1 + ww <= width {
            current.push(' ');
            current.push_str(word);
            current_w += 1 + ww;
        } else {
            lines.push(current);
            if ww <= width {
                current = word.to_string();
                current_w = ww;
            } else {
                let pieces = hard_break(word, width);
                if pieces.len() > 1 {
                    lines.extend(pieces[..pieces.len() - 1].iter().cloned());
                }
                if let Some(last) = pieces.last() {
                    current = last.clone();
                    current_w = disp_width(&current);
                } else {
                    current = String::new();
                    current_w = 0;
                }
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Render a too-wide table as vertical `Header: value` rows.
/// Mirrors `_render_vertical` in Python.
fn render_vertical(rows: &[Vec<String>], ncols: usize, available_width: usize) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut headers = rows[0].clone();
    if headers.len() < ncols {
        headers.extend(std::iter::repeat(String::new()).take(ncols - headers.len()));
    }
    let body = &rows[1..];

    let labels: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            if h.is_empty() {
                format!("Column {}", i + 1)
            } else {
                h.clone()
            }
        })
        .collect();

    let sep_width = if available_width == 0 {
        30
    } else {
        std::cmp::max(20, std::cmp::min(40, available_width.saturating_sub(2)))
    };
    let separator = "─".repeat(sep_width);
    let indent = "  ";
    let indent_w = disp_width(indent);

    let mut out: Vec<String> = Vec::new();
    for (ri, row) in body.iter().enumerate() {
        if ri > 0 {
            out.push(separator.clone());
        }
        for ci in 0..ncols {
            let label = &labels[ci];
            let value = if ci < row.len() { row[ci].as_str() } else { "" };
            let label_w = disp_width(label);
            let first_budget = std::cmp::max(10, available_width.saturating_sub(label_w + 2));
            let cont_budget = std::cmp::max(10, available_width.saturating_sub(indent_w));
            if value.is_empty() {
                out.push(format!("{}:", label));
                continue;
            }
            let wrapped = wrap_to_width(value, first_budget);
            out.push(format!("{}: {}", label, wrapped[0]));
            if wrapped.len() > 1 {
                let cont_text = wrapped[1..].join(" ");
                for cl in wrap_to_width(&cont_text, cont_budget) {
                    if !cl.trim().is_empty() {
                        out.push(format!("{}{}", indent, cl));
                    }
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Public entry — mirrors `realign_markdown_tables` in Python
// ---------------------------------------------------------------------------

/// Rewrite every `| ... |` + divider block with wcwidth-aware padding.
///
/// Lines that are not part of a recognised table are returned verbatim,
/// so this is safe to apply to arbitrary assistant prose.
///
/// If `available_width` is given (terminal cells available for the rendered
/// table), tables wider than that are rendered as vertical key-value pairs
/// instead of a horizontal pipe-bordered grid.
/// Mirrors `realign_markdown_tables` in Python.
pub fn realign_markdown_tables(text: &str, available_width: Option<usize>) -> String {
    if !text.contains('|') {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    let n = lines.len();

    while i < n {
        let line = lines[i];
        if line.contains('|') && i + 1 < n && is_table_divider(lines[i + 1]) {
            let header = split_table_row(line);
            let mut body: Vec<Vec<String>> = Vec::new();
            let mut j = i + 2;
            while j < n && lines[j].contains('|') && !lines[j].trim().is_empty() {
                if is_table_divider(lines[j]) {
                    j += 1;
                    continue;
                }
                body.push(split_table_row(lines[j]));
                j += 1;
            }
            let header_has_content = header.iter().any(|c| !c.is_empty());
            if header_has_content || !body.is_empty() {
                let mut all = Vec::with_capacity(1 + body.len());
                all.push(header);
                all.extend(body);
                out.extend(render_block(all, available_width));
                i = j;
                continue;
            }
        }
        out.push(line.to_string());
        i += 1;
    }

    out.join("\n")
}
