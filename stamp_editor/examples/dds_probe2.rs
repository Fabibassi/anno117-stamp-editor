fn main() {
    let bytes =
        std::fs::read(r"C:\Users\fabi_\Projects\anno117-stamp-editor\_gamedata\_sample.dds").unwrap();
    let dds = image_dds::ddsfile::Dds::read(&mut std::io::Cursor::new(&bytes)).unwrap();
    match image_dds::image_from_dds(&dds, 0) {
        Ok(img) => println!("ok {}x{}", img.width(), img.height()),
        Err(e) => println!("err {e}"),
    }
}
