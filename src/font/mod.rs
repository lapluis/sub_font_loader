pub mod index;
mod names;

pub use names::{
    FontAlias, FontFaceAnalysis, FontFileAnalysis, FontName, analyze_font_data, analyze_font_file,
    analyze_font_files,
};
