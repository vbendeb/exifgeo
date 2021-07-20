//use byteorder::{BigEndian, ByteOrder};
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Result, Seek, SeekFrom};
use std::{char, env, fmt, str};
//use std::mem::MaybeUninit;
use std::slice;

const SOI: u16 = 0xffd8; // Start Of Image.
const SOS: u16 = 0xffda; // Start Of Scan.
const APP1: u16 = 0xffe1; // APP1 marker.
const GPS: u16 = 0x8825; // GPS data.

// GPS directory tags of interest.
const LAT_Q: u16 = 1; // Latitude quadrant.
const LAT_V: u16 = 2; // Latitude value.
const LONG_Q: u16 = 3; // Longtitude quadrant.
const LONG_V: u16 = 4; // Longtitude value;
const TIMESTAMP: u16 = 7; // GPS timestamp.
const DATESTAMP: u16 = 0x1d; // GPS Date.

const NUM_ESSENTIAL_ENTRIES: usize = 6;

struct Coordinate {
    quadrant: char,
    value: f64,
}

fn floats_from_rational(buf: &mut BufReader, offset: u32, floats: &mut [f64]) -> Result<()> {
    let mut rational = [0u8; 24];
    let mut i: usize = 0;

    if floats.len() != 3 {
        return Err(Error::from(ErrorKind::InvalidData));
    }

    buf.save_cursor();
    buf.set_cursor(offset as usize)?;
    buf.read(&mut rational)?;
    buf.restore_cursor()?;
    while i < floats.len() {
        let mut u32v = [0u8; 4];

        u32v.copy_from_slice(&rational[i * 8..i * 8 + 4]);
        let num: u32 = u32::from_le_bytes(u32v);
        u32v.copy_from_slice(&rational[i * 8 + 4..i * 8 + 8]);
        let denom: u32 = u32::from_le_bytes(u32v);
        floats[i] = num as f64 / denom as f64;
        i += 1;
    }
    Ok(())
}

impl Coordinate {
    pub fn new() -> Self {
        Self {
            quadrant: 'x',
            value: 0.0,
        }
    }

    pub fn get_from_ifd(&mut self, buf: &mut BufReader, offset: u32) -> Result<()> {
        let mut floats = [0f64; 3];

        floats_from_rational(buf, offset, &mut floats)?;
        let value: u64 = ((floats[0] + (floats[1] * 60.0 + floats[2]) / 3600.0) * 100000.0) as u64;
        self.value = value as f64 / 100000.0;

        Ok(())
    }
}

struct GpsInfo {
    file_name: String,
    lat: Coordinate,
    longt: Coordinate,
    time: u64,
}

fn get_num(bytes: &[u8]) -> Result<u64> {
    let the_string = match str::from_utf8(bytes) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("failed to convert to string {:?}", bytes);
            return Err(Error::from(ErrorKind::InvalidData));
        }
    };
    let the_number: u64 = match the_string.parse() {
        Ok(v) => v,
        Err(_) => {
            eprintln!("failed to convert to number {}", the_string);
            return Err(Error::from(ErrorKind::InvalidData));
        }
    };

    Ok(the_number)
}

impl GpsInfo {
    pub fn new() -> Self {
        Self {
            file_name: "".to_string(),
            lat: Coordinate::new(),
            longt: Coordinate::new(),
            time: 0,
        }
    }

    pub fn process_timestamp(&mut self, buf: &mut BufReader, offset: u32) -> Result<()> {
        let mut floats = [0f64; 3];

        floats_from_rational(buf, offset, &mut floats)?;
        self.time += (floats[0] * 3600.0 + floats[1] * 60.0 + floats[2]) as u64;

        Ok(())
    }

    pub fn process_datestamp(&mut self, buf: &mut BufReader, offset: u32) -> Result<()> {
        // Date is expressed in form "YYYY:MM:DD"
        let mut date = [0u8; 10];

        buf.save_cursor();
        buf.set_cursor(offset as usize)?;
        buf.read(&mut date)?;
        buf.restore_cursor()?;

        let year = get_num(&date[0..4])?;
        let month = get_num(&date[5..7])?;
        let day = get_num(&date[8..10])?;

        // Let's consider all month have 31 days.
        self.time += year * 31 * 12 * 24 * 60 * 60;
        self.time += (month - 1) * 31 * 24 * 60 * 60;
        self.time += (day - 1) * 24 * 60 * 60;

        Ok(())
    }
}

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
    cursor_stack: Vec<usize>,
    cursor: usize,
    buffer: Vec<u8>,
}

static mut WAYPOINTS: Vec<GpsInfo> = Vec::new();

impl BufReader {
    pub fn init(&mut self, mut f: &File, size: usize) -> Result<()> {
        self.buffer = vec![0u8; size];
        f.read_exact(&mut self.buffer)
    }

    #[allow(dead_code)]
    pub fn dump(&self, num: usize) {
        let mut i: usize = 0;

        while i < num {
            print!(" {:02x}", self.buffer[self.cursor + i]);
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

    pub fn save_cursor(&mut self) {
        self.cursor_stack.push(self.cursor);
    }

    pub fn restore_cursor(&mut self) -> Result<()> {
        match self.cursor_stack.pop() {
            Some(v) => {
                self.cursor = v;
                Ok(())
            }
            None => Err(Error::from(ErrorKind::UnexpectedEof)),
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
            "tag: {:04x}, type: {}, count {}, offset {}",
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

fn process_gps_section(buffer: &mut BufReader, name: &String) -> Result<()> {
    let num_entries = read_u16(buffer)?;
    let mut i: u16 = 0;
    let mut essentials: usize = 0;
    let mut waypoint: GpsInfo = GpsInfo::new();

    while i < num_entries {
        let entry = read_struct::<IfdEntry, BufReader>(buffer)?;

        essentials += 1;
        match entry.tag {
            LAT_Q => match char::from_u32(entry.offset) {
                Some(c) => waypoint.lat.quadrant = c,
                None => {
                    eprintln!("Invalid latitued quadrant");
                    return Err(Error::from(ErrorKind::InvalidData));
                }
            },
            LONG_Q => match char::from_u32(entry.offset) {
                Some(c) => waypoint.longt.quadrant = c,
                None => {
                    eprintln!("Invalid longitude quadrant");
                    return Err(Error::from(ErrorKind::InvalidData));
                }
            },
            LAT_V => waypoint.lat.get_from_ifd(buffer, entry.offset)?,
            LONG_V => waypoint.longt.get_from_ifd(buffer, entry.offset)?,
            TIMESTAMP => waypoint.process_timestamp(buffer, entry.offset)?,
            DATESTAMP => waypoint.process_datestamp(buffer, entry.offset)?,
            _ => essentials -= 1,
        }
        i += 1;
    }
    if essentials == NUM_ESSENTIAL_ENTRIES {
        waypoint.file_name = name.to_string();
        unsafe { WAYPOINTS.push(waypoint) };
        Ok(())
    } else {
        eprintln!("Missing essential GPS entry/ies");
        Err(Error::from(ErrorKind::InvalidData))
    }
}

#[allow(safe_packed_borrows)]
fn handle_app1(f: &mut File, len: u16, name: &String) -> Result<()> {
    const ADVANCE: u16 = 6;
    f.seek(SeekFrom::Current(ADVANCE as i64))?;
    let mut buffer = BufReader {
        cursor_stack: Vec::new(),
        cursor: 0,
        buffer: Vec::new(),
    };

    buffer.init(&f, (len - ADVANCE) as usize)?;
    let eb = read_struct::<ExifBody, BufReader>(&mut buffer)?;
    if !eb.is_valid() {
        eprintln!("Bad  exif header: {}", eb);

        let err = ErrorKind::InvalidData;
        return Err(Error::from(err));
    }

    let mut num_entries = read_u16(&mut buffer)?;
    while num_entries != 0 {
        let entry = read_struct::<IfdEntry, BufReader>(&mut buffer)?;
        if entry.tag == GPS {
            buffer.set_cursor(entry.offset as usize)?;
            process_gps_section(&mut buffer, name)?;
            return Ok(());
        }
        num_entries = num_entries - 1;
    }
    eprintln!("No GPS section found");
    Err(Error::from(ErrorKind::InvalidData))
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
                handle_app1(&mut f, len, name)?;
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
