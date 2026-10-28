// Standalone generator. See README.md; never add this crate to the workspace.
use tantivy::merge_policy::NoMergePolicy;
use tantivy::schema::{
    INDEXED, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions,
};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer};
use tantivy::{Index, doc};

fn main() -> tantivy::Result<()> {
    let destination = std::env::args_os().nth(1).expect("output directory");
    let destination = std::path::Path::new(&destination);
    // Require a new directory: regeneration must not overwrite an existing index.
    std::fs::create_dir(destination)?;
    let mut b = Schema::builder();
    b.add_text_field("path", STRING | STORED);
    b.add_text_field("container", STRING | STORED);
    b.add_text_field("zip_entry", STRING | STORED);
    b.add_text_field("favorite_id", STRING | STORED);
    b.add_text_field("kind", STRING | STORED);
    b.add_i64_field("mtime", INDEXED | STORED);
    b.add_i64_field("file_size", STORED);
    let text_options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("mimv_bigram")
                .set_index_option(IndexRecordOption::WithFreqs),
        )
        .set_stored();
    for name in [
        "name",
        "exif_text",
        "xmp_tweet_text",
        "png_prompt_text",
        "pdf_meta_text",
        "video_meta_text",
        "sidecar_text",
        "tags",
    ] {
        b.add_text_field(name, text_options.clone());
    }
    let schema = b.build();
    let index = Index::create_in_dir(destination, schema.clone())?;
    index.tokenizers().register(
        "mimv_bigram",
        TextAnalyzer::builder(NgramTokenizer::new(2, 2, false)?)
            .filter(LowerCaser)
            .build(),
    );
    let mut writer = index.writer_with_num_threads(1, 64 * 1024 * 1024)?;
    writer.set_merge_policy(Box::new(NoMergePolicy));
    let f = |name| schema.get_field(name).unwrap();
    for (path, name, place) in [
        ("c:/fixture/sakura.jpg", "桜 青空.jpg", "東京 公園"),
        ("c:/fixture/rain.jpg", "雨 夜景.jpg", "大阪 雨"),
        ("c:/fixture/mountain.jpg", "山 青空.jpg", "長野 森林"),
    ] {
        writer.add_document(doc!(
            f("path") => path, f("container") => "fs", f("zip_entry") => "",
            f("favorite_id") => "11111111-1111-4111-8111-111111111111",
            f("kind") => "image", f("mtime") => 100i64, f("file_size") => 1024i64,
            f("name") => name, f("exif_text") => place,
            f("xmp_tweet_text") => "xmp 東京", f("png_prompt_text") => "blue sky",
            f("pdf_meta_text") => "pdf 青空", f("video_meta_text") => "video 青空",
            f("sidecar_text") => "sidecar 青空", f("tags") => "tag 青空",
        ))?;
    }
    writer.commit()?;
    writer.wait_merging_threads()?;
    Ok(())
}
