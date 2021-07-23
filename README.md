# Exif Geotag Processor

This repository contains a Rust program which allows to scan passed in Exif
files (`.jpg`, etc.) and generate a `.gpx` file which can be uploaded to
Google Maps to show the track along which the photos were taken.

Note that this is one of the first programs written by the author in this
language, critiques and improvement suggestions are welcome.

To run this program one needs the `Rust` development environment installed. To
create an executable run
```
$ cargo build
```
which should result in creating of the executable `target/debug/exifgeo`

The following command line options are supported:
```
Usage: target/debug/exifgeo [options] exif_files...

Options:
    -m, --map_name      Name of the generated map, REQUIRED
    -o, --output_file   Output file name, console by default
    -h, --help          Print this help menu
```

`exif_files` is the list of photos to be scanned to retrieve the geotags.
Files which are not valid EXIF files or are missing geotags are reported to
`stderr` and otherwise ignored.

All retrieved geoptags are sorted by timestamp and then a `.gpx` XML file is
generated representing the track in the format recognizable by Google Maps.

To create a customized Google Map do the following as of this writing (July 2021):
- open Google Drive window in a browser
- click on `+New -> More -> Google My Maps`
- in thhe opened page click on `Import` and upload the generated `.gpx` file
