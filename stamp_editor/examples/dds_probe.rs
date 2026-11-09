use image::GenericImageView;

fn main() {
    match image::open(r"C:\Users\fabi_\Projects\anno117-stamp-editor\_gamedata\_sample.dds") {
        Ok(im) => println!("ok {:?} {:?}", im.dimensions(), im.color()),
        Err(e) => println!("err {e}"),
    }
}
