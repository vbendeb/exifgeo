//use byteorder::{BigEndian, ByteOrder};
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Result, Seek, SeekFrom};
use std::{env, fmt, mem};
//use std::mem::MaybeUninit;
use std::slice;

const SOI: u16 = 0xffd8; // Start Of Image.
const SOS: u16 = 0xffda; // Start Of Scan.
const APP1: u16 = 0xffe1; // APP1 marker.
const GPS: u16 = 0x8825; // GPS data.

#[repr(C)]
#[repr(packed)]
struct ExifBody {
    exif: u32,
    zeros: u16,
    tiff: u16,
    size: u16,
    offset: u32,
}

#[repr(C)]
#[repr(packed)]
struct IfdEntry {
    tag: u16,
    typ_e: u16,
    count: u32,
    offset: u32,
}

impl ExifBody {
    fn is_valid(&self) -> bool {
        self.exif == 0x66697845 && self.zeros == 0 && self.tiff == 0x4949 && self.offset == 8
    }
}

fn str_len<T>() -> usize {
    ::std::mem::size_of::<T>()
}

impl fmt::Display for IfdEntry {
    #[allow(safe_packed_borrows)]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "tag: {:x}, type: {}, count {}, offset {}",
            self.tag, self.typ_e, self.count, self.offset
        )
    }
}

impl fmt::Display for ExifBody {
    #[allow(safe_packed_borrows)]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "exif: {:x}, zeros: {}, tiff {:x}, size {}, offset {}",
            self.exif, self.zeros, self.tiff, self.size, self.offset
        )
    }
}

#[allow(deprecated)]
fn read_struct<T>(mut f: &File) -> Result<T> {
    let num_bytes = str_len::<T>();
    unsafe {
        let mut s = ::std::mem::uninitialized();
        let buffer = slice::from_raw_parts_mut(&mut s as *mut T as *mut u8, num_bytes);
        match f.read_exact(buffer) {
            Ok(()) => Ok(s),
            Err(e) => {
                ::std::mem::forget(s);
                Err(e)
            }
        }
    }
}

/*
fn read_struct<T>(mut f: &File) -> Result<T> {
    let num_bytes = ::std::mem::size_of::<T>();
    unsafe {
        let mut s = MaybeUninit::<T>::uninit();
        let buffer = slice::from_raw_parts_mut(s.as_mut_ptr() as *mut u8, num_bytes);
        match f.read_exact(buffer) {
            Ok(()) => Ok(*s.as_mut_ptr()),
            Err(e) => {
                ::std::mem::forget(s);
                Err(e)
            }
        }
    }
}
*/

fn read_u16(mut f: &File) -> Result<u16> {
    let mut tag = [0u8; 2];
    f.read(&mut tag)?;
    Ok(u16::from_le_bytes(tag))
}

fn read_tag(mut f: &File) -> Result<u16> {
    let mut tag = [0u8; 2];
    f.read(&mut tag)?;
    Ok(u16::from_be_bytes(tag))
}

#[allow(safe_packed_borrows)]
fn handle_app1(mut f: &File, len: u16) -> Result<()> {
    let mut togo = len;

    let eb = read_struct::<ExifBody>(f)?;
    togo = togo - str_len::<ExifBody>() as u16;

    if !eb.is_valid() {
        eprintln!(
            "Bad  exif header: {:x}, zeros {}, tiff {:x}, size {}, offset {}",
            eb.exif, eb.zeros, eb.tiff, eb.size, eb.offset
        );
        let err = ErrorKind::InvalidData;
        return Err(Error::from(err));
    }
    println!("Tiff header {}", eb);

    let mut num_entries = read_u16(f)?;
    togo = togo - mem::size_of_val(&num_entries) as u16;

    println!("Found {} directory entries", num_entries);
    while num_entries != 0 {
        let entry = read_struct::<IfdEntry>(f)?;
        togo = togo - str_len::<IfdEntry>() as u16;
        if entry.tag == GPS {
            println!("  {}", entry);
            let seek_for:i64 = (entry.offset - 8 + togo as u32 - len as u32) as i64;
            f.seek(SeekFrom::Current(seek_for))?;
            let gps_tag = read_tag(f)?;
            println!("advanced for {} got tag {:x}", seek_for, gps_tag);
        }
        num_entries = num_entries - 1;
    }
    Ok(())
}

fn parse_file(name: &String) -> Result<()> {
    let mut f = File::open(name)?;
    let err = Err(Error::from(ErrorKind::InvalidData));

    let t = read_tag(&f)?;
    if t != SOI {
        eprintln!("File {} does not seem to be a photo image file ", name);
        return err;
    }

    print!("{}: ", name);
    loop {
        let t = read_tag(&f)?;

        if t == SOS {
            break;
        }
        let len = read_tag(&f)? - 2;

        match t {
            APP1 => {
                handle_app1(&f, len)?;
                return Ok(());
            },
            _ => {
                f.seek(SeekFrom::Current(i64::from(len)))?;
                print!("{:x}:{}: ", t, len + 2);
            }
        }
    }
    err
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    for f in &args[1..] {
        parse_file(f)?;
    }
    Ok(())
}
