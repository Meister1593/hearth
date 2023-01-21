use msdfgen::{Bitmap, Range, Rgb};
use rect_packer::Packer;
use ttf_parser::{Face, GlyphId};
use crate::error::{FontError, FontResult, GlyphShapeError};
use crate::glyph_bitmap::GlyphBitmap;

pub struct GlyphInfo {
    pub width: usize,
    pub height: usize,
    pub anchor: (usize, usize),
}

pub struct GlyphAtlas {
    pub bitmap: GlyphBitmap,
    pub glyphs: Vec<Option<GlyphInfo>>,
}

impl GlyphAtlas {
    pub const SCALE: f64 = 0.02;
    pub const RANGE: Range<f64> = Range::Px(0.05);
    pub const ANGLE_THRESHOLD: f64 = 0.3;
    pub const PACKING_BORDER: usize = 1;
    pub const PACKING_PADDING: usize = 3;
    /// turns a face into a glyph atlas.
    /// all fonts have some glyph shape errors for some reason, we pass those through, as we treat them as non-fatal errors.
    pub fn new(face: &Face) -> FontResult<(GlyphAtlas, Vec<GlyphShapeError>)> {
        let mut glyphs = vec![];
        let mut glyph_shape_errors = vec![];
        for c in 0..face.number_of_glyphs() {
            let glyph = GlyphBitmap::new(Self::SCALE, Self::RANGE, Self::ANGLE_THRESHOLD, &face, GlyphId(c));
            match glyph {
                Ok(glyph) => {
                    glyphs.push(Some(glyph));
                }
                Err(err) => {
                    match err {
                        FontError::GlyphShape(glyph_shape_error) => {
                            glyph_shape_errors.push(glyph_shape_error);
                        }
                        error => return Err(error),
                    }
                    glyphs.push(None);
                }
            }
        }
        let mut packer = Self::generate_packer(&glyphs);
        let width = packer.config().width as usize;
        let height = packer.config().height as usize;
        let mut final_map = Bitmap::<Rgb<f32>>::new(width as u32, height as u32);
        let mut glyph_info = vec![];
        for glyph in glyphs {
            match glyph {
                None => {
                    glyph_info.push(None);
                }
                Some(glyph) => {
                    if let Some(rect) = packer.pack(glyph.width as i32, glyph.height as i32, false) {
                        glyph.copy_into_bitmap(&mut final_map, rect.x as usize, rect.y as usize, width as usize);
                        glyph_info.push(Some(
                            GlyphInfo {
                                width: glyph.width,
                                height: glyph.height,
                                anchor: (rect.x as usize, rect.y as usize),
                            }
                        ))
                    }
                }
            }
        }
        let bitmap = GlyphBitmap {
            data: final_map,
            width,
            height
        };
        Ok((GlyphAtlas {
            bitmap,
            glyphs: glyph_info
        }, glyph_shape_errors))
    }
    fn generate_packer(glyphs: &Vec<Option<GlyphBitmap>>) -> Packer {
        let mut config = rect_packer::Config {
            width: 4,
            height: 4,
            border_padding: Self::PACKING_BORDER as i32,
            rectangle_padding: Self::PACKING_PADDING as i32,
        };
        let mut packer = Packer::new(config);
        let mut last_switched_width = false;
        loop {
            let mut flag = true;
            for glyph in glyphs {
                match glyph {
                    None => {}
                    Some(glyph) => {
                        match packer.pack(glyph.width as i32, glyph.height as i32, false) {
                            None => {
                                match last_switched_width {
                                    true => {
                                        last_switched_width = false;
                                        config.height *= 2;
                                    }
                                    false => {
                                        last_switched_width = true;
                                        config.width *= 2;
                                    }
                                }
                                packer = Packer::new(config);
                                flag = false;
                                break;
                            }
                            Some(_) => {}
                        }
                    }
                }
            }
            if flag {
                return Packer::new(config);
            }
        }
    }
}