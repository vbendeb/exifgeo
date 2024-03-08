extern crate getopts;
use arrayvec::ArrayVec;
use getopts::Options;
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Result, Seek, SeekFrom, Write};
use std::{char, env, fmt, slice, str};

const SOI: u16 = 0xffd8; // Start Of Image.
const SOS: u16 = 0xffda; // Start Of Scan.
const APP1: u16 = 0xffe1; // APP1 marker.
const GPS: u16 = 0x8825; // GPS data.

// GPS directory tags of interest.
const LAT_Q: u16 = 1; // Latitude quadrant.
const LAT_V: u16 = 2; // Latitude value.
const LONG_Q: u16 = 3; // Longitude quadrant.
const LONG_V: u16 = 4; // Longitude value;
const TIMESTAMP: u16 = 7; // GPS timestamp.
const DATESTAMP: u16 = 0x1d; // GPS Date.

const NUM_ESSENTIAL_ENTRIES: usize = 6;

// When running in test mode stack size is reduced.
#[cfg(not(test))]
type AV = ArrayVec<u8, 1_000_000>;
#[cfg(test)]
type AV = ArrayVec<u8, 1_000>;

fn floats_from_rational(buf: &mut BufReader, offset: u32, floats: &mut [f64]) -> Result<()> {
    let mut rational = [0u8; 24];
    let mut i: usize = 0;

    if floats.len() != 3 {
        return Err(Error::from(ErrorKind::InvalidData));
    }

    buf.save_cursor();
    buf.set_cursor(offset as usize)?;
    buf.read(&mut rational)?;
    buf.restore_cursor();
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

fn f64_from_ifd(buf: &mut BufReader, offset: u32) -> Result<f64> {
    let mut floats = [0f64; 3];

    floats_from_rational(buf, offset, &mut floats)?;
    let value: u64 = ((floats[0] + (floats[1] * 60.0 + floats[2]) / 3600.0) * 100000.0) as u64;

    Ok(value as f64 / 100000.0)
}

struct GpsInfo {
    file_name: String,
    lat: f64,
    lon: f64,
    time: u64,
}

impl fmt::Display for GpsInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "file: {} {} {} {}",
            self.file_name, self.lat, self.lon, self.time
        )
    }
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
            lat: 0.0,
            lon: 0.0,
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
        buf.restore_cursor();

        let year = get_num(&date[0..4])?;
        let month = get_num(&date[5..7])?;
        let day = get_num(&date[8..10])?;

        // Let's consider all months have 31 days.
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

impl ExifBody {
    fn tiff(&self) -> u16 {
        self.tiff
    }

    fn size(&self) -> u16 {
        self.size
    }

    fn offset(&self) -> u32 {
        self.offset
    }
}

#[repr(C)]
#[repr(packed)]
struct IfdEntry {
    tag: u16,
    typ_e: u16,
    count: u32,
    offset: u32,
}

impl IfdEntry {
    fn tag(&self) -> u16 {
        self.tag
    }

    fn typ_e(&self) -> u16 {
        self.typ_e
    }

    fn count(&self) -> u32 {
        self.count
    }

    fn offset(&self) -> u32 {
        self.offset
    }
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

    pub fn restore_cursor(&mut self) {
        self.cursor = self.cursor_stack.pop().expect("cursor stack is empty!");
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
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "tag: {:04x}, type: {}, count {}, offset {}",
            self.tag(),
            self.typ_e(),
            self.count(),
            self.offset()
        )
    }
}

impl fmt::Display for ExifBody {
    #[allow(unaligned_references)]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "tiff {:x}, size {}, offset {}",
            self.tiff(),
            self.size(),
            self.offset()
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
    let mut lat_sign: f64 = 1.0;
    let mut lon_sign: f64 = 1.0;

    waypoint.file_name = name.to_string();
    while i < num_entries {
        let entry = read_struct::<IfdEntry, BufReader>(buffer)?;

        essentials += 1;
        match entry.tag {
            LAT_Q => {
                let c = char::from_u32(entry.offset).expect("Bad lat_q value");
                lat_sign = if c == 'S' { -1.0 } else { 1.0 };
            }
            LONG_Q => {
                let c = char::from_u32(entry.offset).expect("Bad long_q value");
                lon_sign = if c == 'W' { -1.0 } else { 1.0 };
            }
            LAT_V => waypoint.lat = f64_from_ifd(buffer, entry.offset)?,
            LONG_V => waypoint.lon = f64_from_ifd(buffer, entry.offset)?,
            TIMESTAMP => waypoint.process_timestamp(buffer, entry.offset)?,
            DATESTAMP => waypoint.process_datestamp(buffer, entry.offset)?,
            _ => essentials -= 1,
        }
        i += 1;
    }
    if essentials == NUM_ESSENTIAL_ENTRIES {
        // Update signs as needed.
        waypoint.lat *= lat_sign;
        waypoint.lon *= lon_sign;

        unsafe { WAYPOINTS.push(waypoint) };
    } else {
        eprintln!("Missing essential GPS entry/ies {}", waypoint);
    }
    Ok(())
}

#[allow(unaligned_references)]
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
        eprintln!("{}: Bad exif header: {}", name, eb);
        return Ok(());
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
    eprintln!("No GPS section found in {}", name);
    Ok(())
}

fn parse_file(name: &String) -> Result<()> {
    println!("Parsing {}", name);
    let mut f = File::open(name)?;

    let t = read_tag(&mut f)?;
    if t != SOI {
        eprintln!("File {} does not seem to be a photo image file ", name);
        return Ok(());
    }

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
            }
        }
    }
    println!("{} done", name);
    Ok(())
}

// GPS Date and time were combined and saved as number of seconds starting on
// Jan 1 0. For simplicity when converting calendar date to this value all
// months were considered to have 31 days. Use this when converting the number
// of seconds back into the real date.
fn print_time(time: u64, av: &mut AV) -> Result<()> {
    let mut run = time;

    let sec = run % 60;
    run /= 60;

    let min = run % 60;
    run /= 60;

    let hour = run % 24;
    run /= 24;

    let day = run % 31 + 1;
    run /= 31;

    let month = run % 12 + 1;
    let year = run / 12;

    write!(
        av,
        "<time>{}-{:02}-{:02}T{:02}:{:02}:{:02}Z</time>",
        year, month, day, hour, min, sec
    )
}

fn print_trackpoint(point: &GpsInfo, av: &mut AV) -> Result<()> {
    write!(av, "<trkpt ")?;
    write!(
        av,
        "lat=\"{:2.5}\" lon=\"{:2.5}\"> ",
        point.lat, point.lon
    )?;
    print_time(point.time, av)?;
    writeln!(av, "</trkpt>")
}

fn print_track(track: &Vec<&GpsInfo>, av: &mut AV, map_name: &String) -> Result<()> {
    writeln!(av, "<trk>")?;
    writeln!(av, "<name>{}</name><number>1</number>", map_name)?;
    writeln!(av, "<trkseg>")?;
    for w in track.iter() {
        print_trackpoint(w, av)?;
    }
    writeln!(av, "</trkseg>")?;
    writeln!(av, "</trk>")
}

fn print_gpx(track: &Vec<&GpsInfo>, av: &mut AV, map_name: &String) -> Result<()> {
    writeln!(
        av,
        "<gpx version=\"1.1\" creator=\"git@github.com:vbendeb/exifgeo.git\">"
    )?;
    writeln!(av, "<name>{}</name>", map_name)?;
    print_track(&track, av, map_name)?;
    writeln!(av, "</gpx>")
}

fn print_xml(av: &mut AV, map_name: &String) -> Result<()> {
    let mut filtered: Vec<&GpsInfo> = Vec::new();

    unsafe {
        WAYPOINTS.sort_by(|a, b| a.time.cmp(&b.time));
        filtered.push(&WAYPOINTS[0]);

        for i in 1..WAYPOINTS.len() {
            if WAYPOINTS[i].time != WAYPOINTS[i - 1].time {
                filtered.push(&WAYPOINTS[i]);
            }
        }
    }
    writeln!(
        av,
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\" ?>"
    )?;
    print_gpx(&filtered, av, map_name)
}

fn prepare_opts() -> Options {
    let mut o = Options::new();

    o.optopt("m", "map_name", "Name of the generated map, REQUIRED", "");
    o.optopt(
        "o",
        "output_file",
        "Output file name, console by default",
        "",
    );
    o.optflag("h", "help", "Print this help menu");
    o
}

fn print_usage(program: &str, o: Options) {
    let brief = format!("Usage: {} [options] exif_files...", program);
    print!("{}", o.usage(&brief));
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let o = prepare_opts();
    let mut base = 3; // If only the required arg is given.

    let matches = match o.parse(&args[1..]) {
        Ok(m) => m,
        Err(f) => {
            eprintln!("{}", f.to_string());
            return Err(Error::from(ErrorKind::InvalidData));
        }
    };

    if matches.opt_present("h") {
        print_usage(&args[0], o);
        return Ok(());
    }

    if !matches.opt_present("m") {
        eprintln!("Error: map name argument is required");
        return Err(Error::from(ErrorKind::InvalidData));
    }

    base += match matches.opt_str("o") {
        Some(_) => 2,
        None => 0,
    };

    for f in &args[base..] {
        parse_file(f)?;
    }

    if unsafe { WAYPOINTS.len() } == 0 {
        println!("No geotags found in input file(s)");
        return Ok(());
    }

    // -n is a required option.
    let map_name = matches.opt_str("m").unwrap();
    let mut buf = AV::new();
    print_xml(&mut buf, &map_name)?;

    let txt = std::str::from_utf8(&buf).unwrap();
    match matches.opt_str("o") {
        Some(name) => {
            let mut f = File::create(name)?;
            f.write(&buf)?;
        }
        None => println!("{}", txt),
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_file() -> Result<()> {
        for i in 0..4 {
            let test_data: String = format!("src/test_data/test{}.jpg", i);

            parse_file(&test_data)?;
        }

        let mut buf: AV = AV::new();
        let map_name = String::from("Test map");
        print_xml(&mut buf, &map_name)?;

        let expected: String =
            fs::read_to_string("src/test_data/result.txt").expect("Failed to read result.txt");

        if expected == std::str::from_utf8(&buf).unwrap() {
            Ok(())
        } else {
            Err(Error::from(ErrorKind::InvalidData))
        }
    }
}
