use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
};

use anyhow::{Context, Result, anyhow};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamilyName, FontStyle, FontWeight, GenericFamily,
    Layout, LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty,
};
use swash::{
    FontRef,
    scale::{Render, ScaleContext, Source, StrikeWith, image::Content},
};

const GLYPH_CACHE_CAPACITY: usize = 4096;

/// Identifies a terminal glyph independently of its cell colors.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct GlyphKey {
    pub(super) text: String,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) underline: bool,
    pub(super) strikeout: bool,
    pub(super) columns: u16,
}

/// Stores a cached monochrome glyph image clipped to its cell span.
#[derive(Clone, Debug)]
pub(super) struct GlyphMask {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) alpha: Vec<u8>,
}

/// Shapes text with Parley and rasterizes its glyphs with Swash.
pub(super) struct FontRenderer {
    font_family: String,
    font_size: f32,
    cell_width: usize,
    cell_height: usize,
    layout_context: LayoutContext<()>,
    font_context: FontContext,
    font_families: Vec<FontFamilyName<'static>>,
    scale_context: ScaleContext,
    glyph_cache: HashMap<GlyphKey, GlyphMask>,
    glyph_order: VecDeque<GlyphKey>,
}

impl FontRenderer {
    /// Creates a font renderer and measures one terminal cell.
    pub(super) fn new(font_size: f32, font_family: impl Into<String>) -> Result<Self> {
        if !font_size.is_finite() || font_size <= 0.0 {
            return Err(anyhow!("font size must be a positive finite number"));
        }

        let font_family = font_family.into();
        let font_families = vec![
            FontFamilyName::Named(Cow::Owned(font_family.clone())),
            FontFamilyName::Generic(GenericFamily::UiMonospace),
            FontFamilyName::Generic(GenericFamily::Monospace),
        ];
        let mut font_context = FontContext::new();
        let mut layout_context = LayoutContext::new();
        let metrics = Self::shape_layout(
            &mut layout_context,
            &mut font_context,
            &font_families,
            "M",
            font_size,
            false,
            false,
            None,
        );
        let line = metrics
            .lines()
            .next()
            .context("measure terminal font cell")?;
        let cell_width = metrics.full_width().round().max(1.0) as usize;
        let cell_height = line.metrics().line_height.round().max(1.0) as usize;

        Ok(Self {
            font_family,
            font_size,
            cell_width,
            cell_height,
            layout_context,
            font_context,
            font_families,
            scale_context: ScaleContext::new(),
            glyph_cache: HashMap::new(),
            glyph_order: VecDeque::new(),
        })
    }

    /// Returns how many cells fit in the requested pixel dimensions.
    pub(super) fn cell_size(&self, width: i32, height: i32) -> (u16, u16) {
        (
            (width.max(1) as usize / self.cell_width).max(1) as u16,
            (height.max(1) as usize / self.cell_height).max(1) as u16,
        )
    }

    /// Returns the measured pixel width of one terminal cell.
    pub(super) fn cell_width(&self) -> usize {
        self.cell_width
    }

    /// Returns the measured pixel height of one terminal cell.
    pub(super) fn cell_height(&self) -> usize {
        self.cell_height
    }

    /// Returns the configured font family for diagnostics.
    pub(super) fn font_family(&self) -> &str {
        &self.font_family
    }

    /// Returns the configured font size for diagnostics.
    pub(super) fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Adds a missing glyph mask to the bounded cache.
    pub(super) fn ensure_glyph(&mut self, key: &GlyphKey) -> Result<()> {
        if self.glyph_cache.contains_key(key) {
            return Ok(());
        }

        let glyph = self.rasterize_glyph(key)?;
        if self.glyph_cache.len() >= GLYPH_CACHE_CAPACITY
            && let Some(expired) = self.glyph_order.pop_front()
        {
            self.glyph_cache.remove(&expired);
        }
        self.glyph_cache.insert(key.clone(), glyph);
        self.glyph_order.push_back(key.clone());
        Ok(())
    }

    /// Returns a cached glyph mask when available.
    pub(super) fn glyph(&self, key: &GlyphKey) -> Option<&GlyphMask> {
        self.glyph_cache.get(key)
    }

    /// Shapes and rasterizes one decorated cell into a reusable monochrome mask.
    fn rasterize_glyph(&mut self, key: &GlyphKey) -> Result<GlyphMask> {
        let width = self.cell_width * usize::from(key.columns);
        let height = self.cell_height;
        let mut alpha = vec![0; width * height];
        let layout = Self::shape_layout(
            &mut self.layout_context,
            &mut self.font_context,
            &self.font_families,
            &key.text,
            self.font_size,
            key.bold,
            key.italic,
            Some(height as f32),
        );
        let block_character = key
            .text
            .chars()
            .next()
            .is_some_and(|character| (0x2580..=0x259f).contains(&(character as u32)));

        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let font = run.font();
                let font_ref = FontRef::from_index(font.data.as_ref(), font.index as usize)
                    .context("load font for glyph rasterization")?;
                let mut scaler = self
                    .scale_context
                    .builder(font_ref)
                    .size(run.font_size())
                    .hint(true)
                    .normalized_coords(run.normalized_coords().iter().copied())
                    .build();
                let renderer = Render::new(&[
                    Source::ColorOutline(0),
                    Source::ColorBitmap(StrikeWith::BestFit),
                    Source::Outline,
                ]);

                for glyph in glyph_run.positioned_glyphs() {
                    let Ok(glyph_id) = u16::try_from(glyph.id) else {
                        continue;
                    };
                    let Some(image) = renderer.render(&mut scaler, glyph_id) else {
                        continue;
                    };
                    let left = glyph.x.floor() as i32 + image.placement.left;
                    let top = glyph.y.round() as i32 - image.placement.top;
                    let image_width = image.placement.width as usize;
                    let image_height = image.placement.height as usize;

                    for image_y in 0..image_height {
                        let y = top + image_y as i32;
                        if y < 0 || y >= height as i32 {
                            continue;
                        }
                        for image_x in 0..image_width {
                            let x = left + image_x as i32;
                            if x < 0 || x >= width as i32 {
                                continue;
                            }
                            let source_index = image_y * image_width + image_x;
                            let channels = match image.content {
                                Content::Mask => 1,
                                Content::SubpixelMask | Content::Color => 4,
                            };
                            let source_index = source_index * channels;
                            let Some(pixel) = image.data.get(source_index..source_index + channels)
                            else {
                                continue;
                            };
                            let mut coverage = match image.content {
                                Content::Mask => pixel[0],
                                Content::SubpixelMask => {
                                    ((u16::from(pixel[0])
                                        + u16::from(pixel[1])
                                        + u16::from(pixel[2]))
                                        / 3) as u8
                                }
                                Content::Color => pixel[3],
                            };
                            if block_character {
                                coverage = if coverage > 0 { 255 } else { 0 };
                            }
                            let destination_index = y as usize * width + x as usize;
                            alpha[destination_index] = alpha[destination_index].max(coverage);
                        }
                    }
                }
            }
        }

        Ok(GlyphMask {
            width,
            height,
            alpha,
        })
    }

    /// Shapes text with the configured system font stack and terminal style.
    fn shape_layout(
        layout_context: &mut LayoutContext<()>,
        font_context: &mut FontContext,
        font_families: &[FontFamilyName<'static>],
        text: &str,
        font_size: f32,
        bold: bool,
        italic: bool,
        line_height: Option<f32>,
    ) -> Layout<()> {
        let mut builder = layout_context.ranged_builder(font_context, text, 1.0, true);
        builder.push_default(StyleProperty::FontFamily(font_families.into()));
        builder.push_default(StyleProperty::FontSize(font_size));
        builder.push_default(StyleProperty::FontWeight(if bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        }));
        builder.push_default(StyleProperty::FontStyle(if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        }));
        if let Some(line_height) = line_height {
            builder.push_default(StyleProperty::LineHeight(LineHeight::Absolute(line_height)));
        }
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        layout
    }
}
