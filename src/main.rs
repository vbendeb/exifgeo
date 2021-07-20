//use byteorder::{BigEndian, ByteOrder};
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Result, Seek, SeekFrom};
use std::{env, fmt};
//use std::mem::MaybeUninit;
use std::slice;

const SOI: u16 = 0xffd8; // Start Of Image.
const SOS: u16 = 0xffda; // Start Of Scan.
const APP1: u16 = 0xffe1; // APP1 marker.
const GPS: u16 = 0x8825; // GPS data.

#[repr(C)]
#[repr(packed)]
struct ExifBody {
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

struct BufReader {
    cursor: usize,
    buffer: Vec<u8>,
}

impl BufReader {
    pub fn init(&mut self, mut f: &File, size: usize) -> Result<()> {
        self.buffer = vec![0u8; size];
        f.read_exact(&mut self.buffer)
    }

    #[allow(dead_code)]
    pub fn dump(&self, num: usize) {
        let mut i: usize = 0;

        while i < num {
            print!(" {:02x}", self.buffer[i]);
            i += 1;
        }
        println!("");
    }

    pub fn set_cursor(&mut self, new_cursor: usize) -> Result<()> {
        if new_cursor >= self.buffer.len() {
            Err(Error::from(ErrorKind::UnexpectedEof))
        } else {
            self.cursor = new_cursor;
            Ok(())
        }
    }
}

impl Read for BufReader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.cursor + buf.len() > self.buffer.len() {
            Err(Error::from(ErrorKind::UnexpectedEof))
        } else {
            buf.copy_from_slice(&self.buffer[self.cursor..self.cursor + buf.len()]);
            self.cursor += buf.len();
            Ok(buf.len())
        }
    }
}

impl ExifBody {
    fn is_valid(&self) -> bool {
        self.tiff == 0x4949 && self.offset == 8
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
            "tiff {:x}, size {}, offset {}",
            self.tiff, self.size, self.offset
        )
    }
}

#[allow(deprecated)]
fn read_struct<T, R: Read>(f: &mut R) -> Result<T> {
    let num_bytes = str_len::<T>();
    unsafe {
        let mut s = ::std::mem::uninitialized();
        let buffer = slice::from_raw_parts_mut(&mut s as *mut T as *mut u8, num_bytes);
        match f.read(buffer) {
            Ok(num) => {
                if num == num_bytes {
                    Ok(s)
                } else {
                    Err(Error::from(ErrorKind::UnexpectedEof))
                }
            }
            Err(e) => {
                ::std::mem::forget(s);
                Err(e)
            }
        }
    }
}

/*
#[allow(deprecated)]
fn read_buf(mut f: &File, num_bytes: usize) -> Result<*mut u8> {
    unsafe {
        let s = ::std::mem::uninitialized();
        let buffer = slice::from_raw_parts_mut(s as *mut u8, num_bytes);
        match f.read_exact(buffer) {
            Ok(()) => Ok(s),
            Err(e) => {
                ::std::mem::forget(s);
                Err(e)
            }
        }
    }
}

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

fn read_u16<T: Read>(f: &mut T) -> Result<u16> {
    let mut tag = [0u8; 2];
    f.read(&mut tag)?;
    Ok(u16::from_le_bytes(tag))
}

fn read_tag<T: Read>(f: &mut T) -> Result<u16> {
    let mut tag = [0u8; 2];
    f.read(&mut tag)?;
    Ok(u16::from_be_bytes(tag))
}

#[allow(safe_packed_borrows)]
fn handle_app1(f: &mut File, len: u16) -> Result<()> {
    println!("Expected len {:x} {}", len, len);
    const ADVANCE: u16 = 6;
    f.seek(SeekFrom::Current(ADVANCE as i64))?;
    let mut buffer = BufReader {
        cursor: 0,
        buffer: vec![0u8; 0],
    };

    buffer.init(&f, (len - ADVANCE) as usize)?;
    let eb = read_struct::<ExifBody, BufReader>(&mut buffer)?;
    if !eb.is_valid() {
        eprintln!("Bad  exif header: {}", eb);

        let err = ErrorKind::InvalidData;
        return Err(Error::from(err));
    }

    let mut num_entries = read_u16(&mut buffer)?;
    println!("Found {} directory entries", num_entries);
    while num_entries != 0 {
        let entry = read_struct::<IfdEntry, BufReader>(&mut buffer)?;
        if entry.tag == GPS {
            println!("  {}", entry);
            buffer.set_cursor(entry.offset as usize)?;
            let gps_tag = read_tag(&mut buffer)?;
            println!("advanced to {} got tag {:x}", entry.offset, gps_tag);
        }
        num_entries = num_entries - 1;
    }

    Ok(())

    /*
        let mut togo = len;

        togo = togo - str_len::<ExifBody>() as u16;

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
    */
}

fn parse_file(name: &String) -> Result<()> {
    let mut f = File::open(name)?;
    let err = Err(Error::from(ErrorKind::InvalidData));

    let t = read_tag(&mut f)?;
    if t != SOI {
        eprintln!("File {} does not seem to be a photo image file ", name);
        return err;
    }

    print!("{}: ", name);
    loop {
        let t = read_tag(&mut f)?;

        if t == SOS {
            break;
        }
        let len = read_tag(&mut f)? - 2;

        match t {
            APP1 => {
                handle_app1(&mut f, len)?;
                return Ok(());
            }
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
