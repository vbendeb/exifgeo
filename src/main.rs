extern crate getopts;
use arrayvec::ArrayVec;
use getopts::Options;
use std::f64::consts::PI;
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Result, Seek, SeekFrom, Write};
use std::{char, env, fmt, slice, str};
use zerocopy::AsBytes;

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
const DISTANCE_DIFF: u32 = 5u32; // Waypoints within 5 m are ignored.
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
        return Err(ErrorKind::InvalidData.into());
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

#[derive(Clone)]
struct GpsInfo {
    lat: f64,
    lon: f64,
    time: u64,
}

impl fmt::Display for GpsInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "file: {} {} {}", self.lat, self.lon, self.time)
    }
}

fn get_num(bytes: &[u8]) -> Result<u64> {
    let the_string = match str::from_utf8(bytes) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("failed to convert to string {:?}", bytes);
            return Err(ErrorKind::InvalidData.into());
        }
    };
    let the_number: u64 = match the_string.parse() {
        Ok(v) => v,
        Err(_) => {
            eprintln!("failed to convert to number {}", the_string);
            return Err(ErrorKind::InvalidData.into());
        }
    };

    Ok(the_number)
}

impl GpsInfo {
    pub fn new() -> Self {
        Self {
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

    // distance in meters.
    fn distance_from(&self, other: &GpsInfo) -> u32 {
        // φ is latitude, λ is longitude, in radians.
        // R is the radius of the globe, 6,371 km
        //a = sin²(Δφ/2) + cos φ1 ⋅ cos φ2 ⋅ sin²(Δλ/2)
        //c = 2 ⋅ atan2( √a, √(1−a) )
        //d = R ⋅ c
        let lat1 = (self.lat * PI) / 180.0;
        let lon1 = (self.lon * PI) / 180.0;
        let lat2 = (other.lat * PI) / 180.0;
        let lon2 = (other.lon * PI) / 180.0;
        let dlat = (lat1 - lat2) / 2.0;
        let dlon = (lon1 - lon2) / 2.0;

        let a = dlat.sin() * dlat.sin() + lat1.cos() * lat2.cos() * dlon.sin() * dlon.sin();
        let sq_a = a.sqrt();
        let sq_1_minus_a = (1.0 - a).sqrt();
        let c = 2.0 * sq_a.atan2(sq_1_minus_a);
        (6371000.0 * c) as u32
    }
}

#[repr(C)]
#[repr(packed)]
#[derive(AsBytes)]
struct ExifBody {
    tiff: u16,
    size: u16,
    offset: u32,
}

impl ExifBody {
    fn tiff(&self) -> u16 {
        u16::from_le_bytes([self.as_bytes()[0], self.as_bytes()[1]])
    }

    fn size(&self) -> u16 {
        u16::from_le_bytes([self.as_bytes()[2], self.as_bytes()[3]])
    }

    fn offset(&self) -> u32 {
        u32::from_le_bytes([
            self.as_bytes()[4],
            self.as_bytes()[5],
            self.as_bytes()[6],
            self.as_bytes()[7],
        ])
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

fn process_gps_section(buffer: &mut BufReader) -> Result<GpsInfo> {
    let num_entries = read_u16(buffer)?;
    let mut i: u16 = 0;
    let mut essentials: usize = 0;
    let mut waypoint: GpsInfo = GpsInfo::new();
    let mut lat_sign: f64 = 1.0;
    let mut lon_sign: f64 = 1.0;

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

        Ok(waypoint)
    } else {
        eprintln!("Missing essential GPS entry/ies {}", waypoint);
        // THis should not stop processing.
        Err(ErrorKind::Other.into())
    }
}

fn handle_app1(f: &mut File, len: u16, name: &str) -> Result<GpsInfo> {
    const ADVANCE: u16 = 6;
    f.seek(SeekFrom::Current(ADVANCE as i64))?;
    let mut buffer = BufReader {
        cursor_stack: Vec::new(),
        cursor: 0,
        buffer: Vec::new(),
    };

    buffer.init(&f, (len - ADVANCE) as usize)?;
    let eb = read_struct::<ExifBody, BufReader>(&mut buffer)?;
    if eb.is_valid() {
        let mut num_entries = read_u16(&mut buffer)?;
        while num_entries != 0 {
            let entry = read_struct::<IfdEntry, BufReader>(&mut buffer)?;
            if entry.tag == GPS {
                buffer.set_cursor(entry.offset as usize)?;
                return process_gps_section(&mut buffer);
            }
            num_entries = num_entries - 1;
        }
        eprintln!("No GPS section found in {}", name);
    } else {
        eprintln!("{}: Bad exif header: {}", name, eb);
    }
    Err(ErrorKind::Other.into())
}

fn parse_file(name: &str) -> Result<GpsInfo> {
    println!("Parsing {}", name);
    let mut f = File::open(name)?;

    let t = read_tag(&mut f)?;
    if t == SOI {
        loop {
            let t = read_tag(&mut f)?;

            if t == SOS {
                break;
            }
            let len = read_tag(&mut f)? - 2;

            match t {
                APP1 => {
                    return handle_app1(&mut f, len, name);
                }
                _ => {
                    f.seek(SeekFrom::Current(i64::from(len)))?;
                }
            }
        }
        println!("{} done", name);
    } else {
        eprintln!("File {} does not seem to be a photo image file ", name);
    }
    Err(ErrorKind::Other.into())
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
    write!(av, "lat=\"{:2.5}\" lon=\"{:2.5}\"> ", point.lat, point.lon)?;
    print_time(point.time, av)?;
    writeln!(av, "</trkpt>")
}

fn print_track(track: &Vec<&GpsInfo>, av: &mut AV, map_name: &str) -> Result<()> {
    writeln!(av, "<trk>")?;
    writeln!(av, "<name>{}</name><number>1</number>", map_name)?;
    writeln!(av, "<trkseg>")?;
    for w in track.iter() {
        print_trackpoint(w, av)?;
    }
    writeln!(av, "</trkseg>")?;
    writeln!(av, "</trk>")
}

fn print_gpx(track: &Vec<&GpsInfo>, av: &mut AV, map_name: &str) -> Result<()> {
    writeln!(
        av,
        "<gpx version=\"1.1\" creator=\"git@github.com:vbendeb/exifgeo.git\">"
    )?;
    writeln!(av, "<name>{}</name>", map_name)?;
    print_track(&track, av, map_name)?;
    writeln!(av, "</gpx>")
}

fn print_xml(av: &mut AV, map_name: &str, waypoints: &Vec<GpsInfo>) -> Result<()> {
    let mut filtered: Vec<&GpsInfo> = Vec::new();
    let mut wp = waypoints.clone();
    wp.sort_by(|a, b| a.time.cmp(&b.time));
    filtered.push(&wp[0]);
    let mut duplicates = 0u32;

    for i in 1..wp.len() {
        // Filter out sequential photos taken closer than 5 meters away from
        // each other.
        if wp[i].distance_from(&wp[i - 1]) > DISTANCE_DIFF {
            filtered.push(&wp[i]);
            continue;
        }
        duplicates += 1;
    }
    writeln!(
        av,
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\" ?>"
    )?;
    println!("dropped {duplicates} duplicate entries");
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
    let mut waypoints: Vec<GpsInfo> = Vec::new();

    let matches = match o.parse(&args[1..]) {
        Ok(m) => m,
        Err(f) => {
            eprintln!("{}", f.to_string());
            return Err(ErrorKind::InvalidData.into());
        }
    };

    if matches.opt_present("h") {
        print_usage(&args[0], o);
        return Ok(());
    }

    if !matches.opt_present("m") {
        eprintln!("Error: map name argument is required");
        return Err(ErrorKind::InvalidData.into());
    }

    base += match matches.opt_str("o") {
        Some(_) => 2,
        None => 0,
    };

    for f in &args[base..] {
        match parse_file(f) {
            Ok(wp) => waypoints.push(wp),
            Err(x) => {
                if x.kind() != ErrorKind::Other {
                    return Err(x);
                }
                println!("some innocuous error");
            }
        };
    }

    if waypoints.len() == 0 {
        println!("No geotags found in input file(s)");
        return Ok(());
    }

    // -n is a required option.
    let map_name = matches.opt_str("m").unwrap();
    let mut buf = AV::new();
    print_xml(&mut buf, &map_name, &waypoints)?;

    let txt = std::str::from_utf8(&buf).unwrap();
    match matches.opt_str("o") {
        Some(name) => {
            if !&name.ends_with(".gpx") {
                println!("Note that mymaps.google.com expects file name to be *.gpx");
            }
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
        let mut waypoints: Vec<GpsInfo> = Vec::new();

        for i in 0..5 {
            let test_data: String = format!("src/test_data/test{}.jpg", i);

            match parse_file(&test_data) {
                Ok(wp) => waypoints.push(wp),
                Err(x) => {
                    if x.kind() != ErrorKind::Other {
                        return Err(x);
                    }
                    println!("some innocuous error");
                }
            };
        }

        let mut buf: AV = AV::new();
        let map_name = String::from("Test map");
        print_xml(&mut buf, &map_name, &waypoints)?;

        let expected: String =
            fs::read_to_string("src/test_data/result.txt").expect("Failed to read result.txt");

        if expected == std::str::from_utf8(&buf).unwrap() {
            Ok(())
        } else {
            println!("result:\n{}\n", std::str::from_utf8(&buf).unwrap());
            println!("expected:\n{}", expected);
            Err(ErrorKind::InvalidData.into())
        }
    }

    #[test]
    fn test_distance_calc() {
        // At 45 degree of longitude one degree is 78,85 km, at the equator it
        // is 111 km, the same as one degree of latitude anywhere. We want to
        // make sure that the resolution is around 5 m.
        let degree_lon_at_45 = 78850.0;
        let degree_lon_at_0 = 111000.0;
        let allowed_delta = 0.004;
        let mut wp0 = GpsInfo::new();
        let mut wp1 = GpsInfo::new();

        // One degree on the equator.
        wp0.lon = 1.0;
        assert!(delta_ratio(degree_lon_at_0, &wp0, &wp1) < allowed_delta);
        wp0.lat = 45.0;
        wp1.lat = 45.0;
        assert!(delta_ratio(degree_lon_at_45, &wp0, &wp1) < allowed_delta);
    }

    fn delta_ratio(base: f64, wp0: &GpsInfo, wp1: &GpsInfo) -> f64 {
        ((base - wp0.distance_from(wp1) as f64) / base).abs()
    }
}
