use std::{fs::File, io::Read};

use flate2::read::GzDecoder;
use xz::read::XzDecoder;

#[test]
pub fn haha() {
    let mut ar = ar::Archive::new(File::open("Muse_Hub.deb").unwrap());
    while let Some(entry) = ar.next_entry() {
        let mut entry = entry.unwrap();
        let id = entry.header().identifier();

        if !id.starts_with(b"control.tar") {
            continue;
        }
        let mut tar = vec![];
        match id {
            b"control.tar.gz" => GzDecoder::new(entry).read_to_end(&mut tar).unwrap(),
            b"control.tar.xz" => XzDecoder::new(entry).read_to_end(&mut tar).unwrap(),
            // it's already a tarball
            b"control.tar" => entry.read_to_end(&mut tar).unwrap(),
            _ => panic!("Unknown control member!"),
        };
        let mut data = String::new();
        tar::Archive::new(tar.as_slice())
            .entries()
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| dbg!(e.path().unwrap()).file_name() == Some("control".as_ref()))
            .unwrap()
            .read_to_string(&mut data)
            .unwrap();

        dbg!(data);
        return;
    }
    panic!("Cannot find control member!");
}
