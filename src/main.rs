use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::BufReader;
use std::os::windows::prelude::OsStringExt;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use clap::{CommandFactory, Parser, ValueEnum};
use colored::Colorize;
use fast_image_resize::{ResizeOptions, Resizer, SrcCropping};
use hex_color::HexColor;
use image::{DynamicImage, GenericImage, ImageReader, Rgb, RgbImage};
use imageproc::rect::Rect;
use inquire::validator::MinLengthValidator;
use pluralizer::pluralize;
use turbojpeg::Subsamp;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QDC_VIRTUAL_MODE_AWARE,
    QueryDisplayConfig,
};
use windows::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE, WIN32_ERROR};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};

use crate::display::{Display, DisplayConfiguration};

mod display;

#[derive(ValueEnum, Debug, Copy, Clone)]
enum ResizeMode {
    /// Fills the entire display with the image. Stretches the image disproportionally as needed
    Stretch,
    /// Fills the entire display with the image. Scales the image proportionally
    Fill,
    /// Fits the entire image into the display. Scales the image proportionally
    Fit,
}

/// A simple program to create wallpapers that span across all monitors from separate images
#[derive(Parser, Debug)]
#[command(about, arg_required_else_help = true)]
struct Args {
    /// Print display information
    #[arg(short = 'd', long = "displays", action)]
    show_displays: bool,
    /// Overwrite output file if it already exists without confirmation
    #[arg(short = 'f', long = "force", action)]
    overwrite: bool,
    /// Name of the output image
    #[arg(short, long, default_value = "wallpaper.jpg", value_parser = output_parser)]
    output: String,
    /// Resize mode to apply if a source image resolution doesn't match display one
    #[arg(short, long, value_enum, default_value_t = ResizeMode::Stretch)]
    mode: ResizeMode,
    /// A list of images or colors in hex (e.g. #FF0000 for red) in order of displays to generate wallpaper from.
    /// Use empty string ("") to skip a display (will use black color instead)
    #[arg(allow_hyphen_values = true)]
    images: Vec<WallpaperArgument>,
}

fn output_parser(name: &str) -> Result<String, String> {
    let lowercase = name.to_lowercase();
    if !lowercase.ends_with(".jpeg") && !lowercase.ends_with(".jpg") {
        return Ok(name.to_owned() + ".jpg");
    }
    Ok(name.to_owned())
}

#[derive(Debug, Clone)]
enum WallpaperArgument {
    Image(Arc<File>, String),
    Color(HexColor),
}

impl FromStr for WallpaperArgument {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(WallpaperArgument::Color(HexColor::BLACK));
        }
        if let Ok(color) = HexColor::parse_rgb(s) {
            return Ok(WallpaperArgument::Color(color));
        }
        if let Ok(file) = File::open(s) {
            return Ok(WallpaperArgument::Image(Arc::new(file), s.to_owned()));
        }
        Err("Unable to parse color or open file")
    }
}

fn main() {
    let mut args = Args::parse();
    if !args.show_displays && args.images.is_empty() {
        let _ = Args::command().print_help();
        return;
    }

    let mut config = get_display_configuration();
    if args.show_displays {
        config.show_displays();
    }
    if args.images.is_empty() {
        return;
    }

    if config.displays.len() != args.images.len() {
        println!(
            "{} Detected {} but you provided {}, please check the arguments and try again.",
            "!".yellow(),
            pluralize("display", config.displays.len() as isize, true),
            pluralize("image", args.images.len() as isize, true)
        );
        if !args.show_displays {
            config.show_displays();
        }
        return;
    }
    while !args.overwrite && Path::new(&args.output).exists() {
        let message = format!(
            "Output file '{}' already exists. Overwrite?",
            args.output.yellow()
        );
        let confirm = inquire::Confirm::new(&message);
        args.overwrite = confirm.prompt().unwrap_or(false);
        if !args.overwrite {
            let input = inquire::Text::new("Please, enter new name for the output wallpaper:")
                .with_validator(MinLengthValidator::new(1));
            args.output = input.prompt().unwrap_or(args.output);
        }
    }
    config.normalize();

    let virtual_resolution = config.bounds.resolution();
    let mut output = RgbImage::new(virtual_resolution.0, virtual_resolution.1);

    for (idx, arg) in args.images.iter_mut().enumerate() {
        let display = config
            .displays
            .get(idx)
            .expect("length of images equals to the one of displays");
        let display_res = display.bounds.resolution();
        match arg {
            WallpaperArgument::Image(file, filename) => {
                let reader = match ImageReader::new(BufReader::new(file)).with_guessed_format() {
                    Ok(reader) => reader,
                    Err(err) => {
                        println!(
                            "{} Unable to detect image format for '{}': {}",
                            "!".yellow(),
                            filename,
                            err
                        );
                        continue;
                    }
                };
                let image = match reader.decode() {
                    Ok(image) => image,
                    Err(err) => {
                        println!(
                            "{} Unable to decode image '{}': {}",
                            "!".yellow(),
                            filename,
                            err
                        );
                        continue;
                    }
                };

                let mut resizer = Resizer::new();
                let cropping = match args.mode {
                    ResizeMode::Stretch => SrcCropping::None,
                    ResizeMode::Fill => SrcCropping::FitIntoDestination((0.5, 0.5)),
                    ResizeMode::Fit => SrcCropping::None,
                };
                let dest_res = match args.mode {
                    ResizeMode::Stretch | ResizeMode::Fill => (display_res.0, display_res.1),
                    ResizeMode::Fit => {
                        let width_ratio = image.width() as f32 / display_res.0 as f32;
                        let height_ratio = image.height() as f32 / display_res.1 as f32;
                        if width_ratio - height_ratio > f32::EPSILON {
                            (
                                display_res.0,
                                (image.height() as f32 / width_ratio).round() as u32,
                            )
                        } else {
                            (
                                (image.width() as f32 / height_ratio).round() as u32,
                                display_res.1,
                            )
                        }
                    }
                };
                let mut destination =
                    DynamicImage::ImageRgb8(RgbImage::new(dest_res.0, dest_res.1));
                if let Err(err) = resizer.resize(
                    &image,
                    &mut destination,
                    &ResizeOptions {
                        cropping,
                        ..Default::default()
                    },
                ) {
                    println!(
                        "{} Unable to resize image '{}': {}",
                        "!".yellow(),
                        filename,
                        err
                    );
                    continue;
                }

                let rgb8 = destination.to_rgb8();
                let mut offset = (display.bounds.min_x as u32, display.bounds.min_y as u32);
                if dest_res.0 < display_res.0 {
                    offset.0 += (display_res.0 - dest_res.0) / 2
                }
                if dest_res.1 < display_res.1 {
                    offset.1 += (display_res.1 - dest_res.1) / 2
                }
                if let Err(err) = output.copy_from(&rgb8, offset.0, offset.1) {
                    println!(
                        "{} Unable to copy image '{}': {}",
                        "!".yellow(),
                        filename,
                        err
                    );
                    continue;
                }
            }
            WallpaperArgument::Color(color) => {
                if HexColor::BLACK.eq(color) {
                    continue;
                }
                imageproc::drawing::draw_filled_rect_mut(
                    &mut output,
                    Rect::at(display.bounds.min_x, display.bounds.min_y)
                        .of_size(display_res.0, display_res.1),
                    Rgb([color.r, color.g, color.b]),
                );
            }
        }
    }
    let picture_compressed = match turbojpeg::compress_image(&output, 100, Subsamp::None) {
        Ok(compressed) => compressed,
        Err(err) => {
            println!("{} {}", "! Unable to compress wallpaper:".red(), err);
            return;
        }
    };

    match std::fs::write(args.output, picture_compressed) {
        Ok(_) => {
            println!("{}", "Done!".green());
        }
        Err(err) => {
            println!("{} {}", "! Unable to save wallpaper:".red(), err);
        }
    };
}

fn get_display_configuration() -> DisplayConfiguration {
    unsafe extern "system" fn callback(
        monitor: HMONITOR,
        _: HDC,
        rect_ptr: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let rect = *rect_ptr;
        let data = lparam.0 as *mut (DisplayConfiguration, HashMap<String, String>);
        let config = &mut (*data).0;
        config.bounds.min_x = config.bounds.min_x.min(rect.left);
        config.bounds.max_x = config.bounds.max_x.max(rect.right);
        config.bounds.min_y = config.bounds.min_y.min(rect.top);
        config.bounds.max_y = config.bounds.max_y.max(rect.bottom);

        let mut monitor_info: MONITORINFOEXW = std::mem::zeroed();
        monitor_info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        let monitor_info_exw_ptr = &mut monitor_info as *mut _ as *mut MONITORINFO;

        let name = match GetMonitorInfoW(monitor, monitor_info_exw_ptr).ok() {
            Ok(_) => match convert_string(&monitor_info.szDevice) {
                Some(str) => (*data)
                    .1
                    .get(&str)
                    .map(|s| s.to_owned())
                    .unwrap_or("Unknown".to_owned()),
                None => "Unknown".to_owned(),
            },
            Err(err) => {
                println!("{} Unable to get monitor info: {}", "!".yellow(), err);
                "Unknown".to_owned()
            }
        };

        config.displays.push(Display {
            name,
            bounds: rect.into(),
        });

        TRUE
    }

    let names = get_monitor_names();

    let mut data = (DisplayConfiguration::default(), names);
    match unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(callback),
            LPARAM(&mut data as *mut _ as isize),
        )
    }
    .ok()
    {
        Ok(_) => data.0,
        Err(err) => {
            panic!("{} {}", "Unable to get display configuration:".red(), err);
        }
    }
}

fn get_monitor_names() -> HashMap<String, String> {
    let flags = QDC_ONLY_ACTIVE_PATHS | QDC_VIRTUAL_MODE_AWARE;
    let mut path_count = 0u32;
    let mut mode_count = 0u32;
    match unsafe {
        GetDisplayConfigBufferSizes(flags, &mut path_count as *mut _, &mut mode_count as *mut _)
    }
    .ok()
    {
        Ok(_) => {}
        Err(err) => {
            println!(
                "{} Unable to get display configuration buffer sizes: {}",
                "!".yellow(),
                err
            );
            return HashMap::new();
        }
    }

    let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::with_capacity(path_count as usize);
    let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = Vec::with_capacity(mode_count as usize);
    unsafe {
        match QueryDisplayConfig(
            flags,
            &mut path_count as *mut _,
            paths.as_mut_ptr(),
            &mut mode_count as *mut _,
            modes.as_mut_ptr(),
            None,
        )
        .ok()
        {
            Ok(_) => {}
            Err(err) => {
                println!("Unable to query display config: {}", err);
                return HashMap::new();
            }
        }
        paths.set_len(path_count as usize);
        modes.set_len(mode_count as usize);
    }

    let mut result: HashMap<String, String> = HashMap::with_capacity(path_count as usize);

    for path in &paths {
        let target_name = unsafe {
            let mut target_name: DISPLAYCONFIG_TARGET_DEVICE_NAME = std::mem::zeroed();
            target_name.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                id: path.targetInfo.id,
                adapterId: path.targetInfo.adapterId,
                size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            };
            let device_name_header_ptr =
                &mut target_name as *mut _ as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER;

            if let Err(err) =
                WIN32_ERROR(DisplayConfigGetDeviceInfo(device_name_header_ptr) as u32).ok()
            {
                println!("Unable to get target name: {}", err);
                continue;
            }

            target_name
        };

        let target_friendly_name = match convert_string(&target_name.monitorFriendlyDeviceName) {
            Some(str) => str.to_owned(),
            None => {
                println!("Unable to parse target friendly name to a UTF-8 string");
                continue;
            }
        };

        let source_name = unsafe {
            let mut source_name: DISPLAYCONFIG_SOURCE_DEVICE_NAME = std::mem::zeroed();
            source_name.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.sourceInfo.id,
            };
            let adapter_name_header_ptr =
                &mut source_name as *mut _ as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER;

            if let Err(err) =
                WIN32_ERROR(DisplayConfigGetDeviceInfo(adapter_name_header_ptr) as u32).ok()
            {
                println!("Unable to get source name: {}", err);
                continue;
            }

            source_name
        };

        let gdi_device_name = match convert_string(&source_name.viewGdiDeviceName) {
            Some(str) => str.to_owned(),
            None => {
                println!("Unable to parse source name to a UTF-8 string");
                continue;
            }
        };

        result.insert(gdi_device_name, target_friendly_name);
    }

    result
}

fn convert_string(vec: &[u16]) -> Option<String> {
    let os_string = match vec.iter().position(|c| *c == 0) {
        Some(len) => OsString::from_wide(&vec[0..len]),
        None => OsString::from_wide(&vec[0..vec.len()]),
    };
    os_string.to_str().map(|s| s.to_owned())
}
