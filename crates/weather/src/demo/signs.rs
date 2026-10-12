//! Printed graphics for the station, painted with the stroke font: the thermometer's dual
//! °C/°F scale, the snow stake's bands and numbers, the rain gauge scale, the station sign,
//! the shelter's name plate and backlit poster, the display's face and the road lettering.

use engine_core::math::Vec2;
use engine_render::Image;

use super::font::{Canvas, text_width};
use super::noise::{fbm, hex, sq, value};
use super::props::{RAIN_TUBE_LEN, THERMO_BOARD, THERMO_SCALE_LEN, THERMO_SCALE_Y0};
use super::texgen::{compose, par_pixels, sign_grime};

/// Pixels per meter of the thermometer board texture.
const THERMO_PPM: f32 = 600.0;

/// Dual °C/°F scale for the thermometer board (covers the board's front face).
pub fn thermometer_scale() -> Image {
    let (bw, bh, by0) = THERMO_BOARD;
    let (w, h) = ((bw * THERMO_PPM) as u32, (bh * THERMO_PPM) as u32);
    let px = |x: f32, y: f32| Vec2::new(x * THERMO_PPM, (bh - y) * THERMO_PPM);
    let y_of_c = |c: f32| THERMO_SCALE_Y0 - by0 + (c + 30.0) / 80.0 * THERMO_SCALE_LEN;
    let mut ink = Canvas::new(w, h);
    let mut blue = Canvas::new(w, h);
    let mut red = Canvas::new(w, h);
    let cx = bw * 0.5;
    // Border.
    let (m, r) = (0.025, 0.012);
    for (a, b) in [
        ((m, m), (bw - m, m)),
        ((bw - m, m), (bw - m, bh - m)),
        ((bw - m, bh - m), (m, bh - m)),
        ((m, bh - m), (m, m)),
    ] {
        blue.line(px(a.0, a.1), px(b.0, b.1), r * THERMO_PPM);
    }
    // Celsius on the left.
    for c in -30..=50 {
        let y = y_of_c(c as f32);
        let (len, wt) = if c % 10 == 0 {
            (0.075, 4.0)
        } else if c % 5 == 0 {
            (0.05, 3.0)
        } else {
            (0.03, 2.0)
        };
        ink.line(px(cx - 0.035, y), px(cx - 0.035 - len, y), wt);
        if c % 10 == 0 {
            let s = c.to_string();
            let cap = 0.05 * THERMO_PPM;
            let tw = text_width(&s) * cap / 6.0;
            let target = if c < 0 {
                &mut blue
            } else if c >= 30 {
                &mut red
            } else {
                &mut ink
            };
            target.text(
                &s,
                px(cx - 0.125, y - 0.025) - Vec2::new(tw, 0.0),
                cap,
                5.0,
                1.0,
            );
        }
    }
    // Fahrenheit on the right.
    for f in (-20..=120).step_by(2) {
        let c = (f as f32 - 32.0) * 5.0 / 9.0;
        let y = y_of_c(c);
        let (len, wt) = if f % 20 == 0 {
            (0.07, 3.5)
        } else if f % 10 == 0 {
            (0.045, 2.5)
        } else {
            (0.025, 1.8)
        };
        ink.line(px(cx + 0.035, y), px(cx + 0.035 + len, y), wt);
        if f % 20 == 0 {
            let cap = 0.034 * THERMO_PPM;
            ink.text(&f.to_string(), px(cx + 0.12, y - 0.017), cap, 3.5, 1.0);
        }
    }
    // Freezing line across the tube backing, unit headers, a small maker's line.
    blue.line(px(cx - 0.11, y_of_c(0.0)), px(cx - 0.04, y_of_c(0.0)), 3.0);
    let cap = 0.085 * THERMO_PPM;
    ink.text("°C", px(0.07, bh - 0.16), cap, 8.0, 1.0);
    ink.text("°F", px(bw - 0.2, bh - 0.16), cap, 8.0, 1.0);
    ink.text_centered(
        "METEO",
        cx * THERMO_PPM,
        (bh - 0.09) * THERMO_PPM,
        0.03 * THERMO_PPM,
        3.0,
    );
    // White backing strip behind the tube.
    let mut strip = Canvas::new(w, h);
    let (s0, s1) = (px(cx - 0.03, bh - 0.2), px(cx + 0.03, 0.08));
    strip.rect(s0.x, s0.y, s1.x, s1.y);
    let bg = |u: f32, v: f32| {
        let g = sign_grime(u, v, 7);
        let k = 0.84 - 0.25 * g;
        [k, k, k * 0.96]
    };
    compose(
        w,
        h,
        &[
            (&strip, [0.95, 0.95, 0.94]),
            (&ink, [0.02, 0.02, 0.025]),
            (&blue, hex(0x1d4f8c)),
            (&red, hex(0xb8202a)),
        ],
        bg,
    )
}

/// Snow stake face (one 6 cm × 2 m side): red and white 10 cm bands, 2 cm ticks, numbers
/// every 10 cm.
pub fn snow_stake() -> Image {
    let (w, h) = (64u32, 2048u32);
    let ppm_y = h as f32 / 2.0;
    let ppm_x = w as f32 / 0.06;
    let py = |m: f32| (2.0 - m) * ppm_y;
    let mut red = Canvas::new(w, h);
    let mut black = Canvas::new(w, h);
    let mut white = Canvas::new(w, h);
    for band in 0..20 {
        let (y0, y1) = (band as f32 * 0.1, band as f32 * 0.1 + 0.1);
        if band % 2 == 0 {
            red.rect(0.0, py(y1), w as f32, py(y0));
        }
    }
    for k in 0..=100 {
        let y = k as f32 * 0.02;
        let len = if k % 5 == 0 { 0.022 } else { 0.012 };
        let ink = if (k / 5) % 2 == 0 && k % 5 != 0 {
            &mut white
        } else {
            &mut black
        };
        ink.line(Vec2::new(0.0, py(y)), Vec2::new(len * ppm_x, py(y)), 3.0);
    }
    for band in 1..20 {
        let label = (band * 10).to_string();
        // Numbers fit between the tick marks and the far edge of the face.
        let cap = 0.014 * ppm_y;
        let aspect = ppm_x / ppm_y * 0.8;
        let tw = text_width(&label) * cap / 6.0 * aspect;
        let y = band as f32 * 0.1 + 0.05 - 0.008;
        let ink = if band % 2 == 0 {
            &mut white
        } else {
            &mut black
        };
        let left = 0.026 * ppm_x;
        let room = w as f32 - left - 0.004 * ppm_x;
        ink.text(
            &label,
            Vec2::new(left + (room - tw) * 0.5, py(y)),
            cap,
            3.5,
            aspect,
        );
    }
    compose(
        w,
        h,
        &[
            (&red, hex(0xc4161c)),
            (&black, [0.015, 0.015, 0.015]),
            (&white, [0.9, 0.9, 0.88]),
        ],
        |u, v| {
            let g = sign_grime(u, v, 3) * 0.6 + (1.0 - v) * 0.0;
            let k = 0.86 - 0.3 * g;
            [k, k, k * 0.97]
        },
    )
}

/// Rain gauge scale plate: 0–30 mm, ticks every mm.
pub fn rain_scale() -> Image {
    let (w, h) = (64u32, 640u32);
    let len = RAIN_TUBE_LEN + 0.02;
    let ppm_y = h as f32 / len;
    let ppm_x = w as f32 / 0.036;
    let mut ink = Canvas::new(w, h);
    for mm in 0..=30 {
        let y = 0.01 + mm as f32 / 30.0 * RAIN_TUBE_LEN;
        let py = (len - y) * ppm_y;
        let l = if mm % 10 == 0 {
            0.016
        } else if mm % 5 == 0 {
            0.011
        } else {
            0.006
        };
        ink.line(Vec2::new(0.0, py), Vec2::new(l * ppm_x, py), 2.5);
        if mm % 10 == 0 {
            ink.text(
                &mm.to_string(),
                Vec2::new(0.018 * ppm_x, py + 0.004 * ppm_y),
                0.008 * ppm_y,
                2.2,
                ppm_x / ppm_y,
            );
        }
    }
    ink.text(
        "MM",
        Vec2::new(0.012 * ppm_x, 0.012 * ppm_y),
        0.006 * ppm_y,
        2.0,
        ppm_x / ppm_y,
    );
    compose(w, h, &[(&ink, [0.02, 0.02, 0.02])], |_, _| {
        [0.86, 0.86, 0.84]
    })
}

/// Station sign (1.4 × 0.44 m): white lettering on dark green with a white border.
pub fn station_sign() -> Image {
    let (w, h) = (1024u32, 322u32);
    let ppm = w as f32 / 1.4;
    let mut white = Canvas::new(w, h);
    let m = 0.025 * ppm;
    let (x1, y1) = (w as f32 - m, h as f32 - m);
    for (a, b) in [
        (Vec2::new(m, m), Vec2::new(x1, m)),
        (Vec2::new(x1, m), Vec2::new(x1, y1)),
        (Vec2::new(x1, y1), Vec2::new(m, y1)),
        (Vec2::new(m, y1), Vec2::new(m, m)),
    ] {
        white.line(a, b, 0.01 * ppm);
    }
    white.text_centered(
        "WEATHER STATION",
        w as f32 * 0.5,
        0.2 * ppm,
        0.095 * ppm,
        0.016 * ppm,
    );
    white.text_centered(
        "ALT 112 M   52.37 N   4.89 E",
        w as f32 * 0.5,
        0.33 * ppm,
        0.04 * ppm,
        0.008 * ppm,
    );
    compose(w, h, &[(&white, [0.86, 0.87, 0.85])], |u, v| {
        let g = sign_grime(u, v, 11);
        let k = 1.0 - 0.5 * g;
        let c = hex(0x1b4a38);
        [c[0] * k, c[1] * k, c[2] * k]
    })
}

/// Name plate on the shelter's front fascia (1.9 × 0.12 m).
pub fn shelter_plate() -> Image {
    let (w, h) = (1024u32, 64u32);
    let ppm = w as f32 / 1.9;
    let mut white = Canvas::new(w, h);
    white.text_centered(
        "WEATHER STATION",
        w as f32 * 0.5,
        0.088 * ppm,
        0.058 * ppm,
        0.0085 * ppm,
    );
    compose(w, h, &[(&white, [0.85, 0.85, 0.83])], |u, v| {
        let k = 0.045 * (1.0 - 0.4 * sign_grime(u, v, 5));
        [k, k * 1.05, k * 1.1]
    })
}

/// Backlit poster (1.06 × 1.78 m): a sky with sun, cloud and rain over a forecast panel.
pub fn poster() -> Image {
    let (w, h) = (512u32, 860u32);
    let ppm = w as f32 / 1.06;
    let p = |x: f32, y: f32| Vec2::new(x * ppm, y * ppm);
    let mut sun = Canvas::new(w, h);
    let mut cloud = Canvas::new(w, h);
    let mut rain = Canvas::new(w, h);
    let mut title = Canvas::new(w, h);
    let mut ink = Canvas::new(w, h);
    let mut accent = Canvas::new(w, h);
    sun.disc(p(0.66, 0.3), 0.13 * ppm);
    for k in 0..12 {
        let a = k as f32 / 12.0 * std::f32::consts::TAU;
        let d = Vec2::new(a.cos(), a.sin());
        sun.line(
            p(0.66, 0.3) + d * 0.17 * ppm,
            p(0.66, 0.3) + d * 0.23 * ppm,
            0.018 * ppm,
        );
    }
    for (x, y, r) in [
        (0.3, 0.46, 0.11),
        (0.44, 0.4, 0.14),
        (0.6, 0.47, 0.1),
        (0.22, 0.52, 0.08),
        (0.4, 0.53, 0.09),
        (0.52, 0.53, 0.08),
    ] {
        cloud.disc(p(x, y), r * ppm);
    }
    for k in 0..7 {
        let x = 0.24 + k as f32 * 0.055;
        rain.line(p(x, 0.66), p(x - 0.03, 0.74), 0.012 * ppm);
    }
    title.text_centered(
        "WEATHER",
        w as f32 * 0.5,
        0.98 * ppm,
        0.1 * ppm,
        0.018 * ppm,
    );
    title.text_centered(
        "STATION",
        w as f32 * 0.5,
        1.13 * ppm,
        0.1 * ppm,
        0.018 * ppm,
    );
    for (i, (day, t)) in [("MON", "12°"), ("TUE", "9°"), ("WED", "14°")]
        .iter()
        .enumerate()
    {
        let x = 0.2 + i as f32 * 0.33;
        ink.text_centered(day, x * ppm, 1.36 * ppm, 0.045 * ppm, 0.009 * ppm);
        accent.text_centered(t, x * ppm, 1.5 * ppm, 0.07 * ppm, 0.013 * ppm);
        ink.ring(p(x, 1.6), 0.04 * ppm, 0.006 * ppm);
    }
    ink.text_centered(
        "LIVE DATA AT THE MAST",
        w as f32 * 0.5,
        1.72 * ppm,
        0.03 * ppm,
        0.006 * ppm,
    );
    let split = 0.82;
    compose(
        w,
        h,
        &[
            (&sun, hex(0xffc933)),
            (&rain, hex(0x2f6fb5)),
            (&cloud, [0.95, 0.96, 0.97]),
            (&title, hex(0x13325a)),
            (&ink, hex(0x23303d)),
            (&accent, hex(0xd8462b)),
        ],
        move |u, v| {
            let y = v * 1.78;
            if y < split {
                let t = y / split;
                let top = hex(0x2e7fd0);
                let bot = hex(0xa9d4f5);
                [
                    top[0] + (bot[0] - top[0]) * t,
                    top[1] + (bot[1] - top[1]) * t,
                    top[2] + (bot[2] - top[2]) * t,
                ]
            } else {
                let n = 0.9 + 0.03 * fbm(u * 4.0, v * 4.0, sq(4), 3, 9);
                [n, n, n * 0.98]
            }
        },
    )
}

/// Size of the bus stop flag plate, meters.
pub const FLAG_SIZE: (f32, f32) = (0.48, 0.66);
/// Size of the timetable sheet, meters.
pub const TIMETABLE_SIZE: (f32, f32) = (0.32, 0.48);

/// Bus stop flag (0.48 × 0.66 m): a bus pictogram on a blue panel, the stop's name and its
/// route numbers on a white enamel plate.
pub fn bus_flag() -> Image {
    let (w, h) = (512u32, 704u32);
    let ppm = w as f32 / FLAG_SIZE.0;
    let p = |x: f32, y: f32| Vec2::new(x * ppm, y * ppm);
    let mut blue = Canvas::new(w, h);
    let mut body = Canvas::new(w, h);
    let mut glass = Canvas::new(w, h);
    let mut ink = Canvas::new(w, h);
    let mut routes = [Canvas::new(w, h), Canvas::new(w, h), Canvas::new(w, h)];
    let mut white_ink = Canvas::new(w, h);
    // Border and the pictogram panel.
    let m = 0.014 * ppm;
    let (x1, y1) = (w as f32 - m, h as f32 - m);
    for (a, b) in [
        (p(0.014, 0.014), Vec2::new(x1, m)),
        (Vec2::new(x1, m), Vec2::new(x1, y1)),
        (Vec2::new(x1, y1), Vec2::new(m, y1)),
        (Vec2::new(m, y1), Vec2::new(m, m)),
    ] {
        blue.line(a, b, 0.008 * ppm);
    }
    let (px0, py0, px1, py1) = (0.04, 0.04, 0.44, 0.3);
    let r = 0.025;
    blue.rect((px0 + r) * ppm, py0 * ppm, (px1 - r) * ppm, py1 * ppm);
    blue.rect(px0 * ppm, (py0 + r) * ppm, px1 * ppm, (py1 - r) * ppm);
    for (cx, cy) in [
        (px0 + r, py0 + r),
        (px1 - r, py0 + r),
        (px0 + r, py1 - r),
        (px1 - r, py1 - r),
    ] {
        blue.disc(p(cx, cy), r * ppm);
    }
    // Bus seen from the side, facing right.
    body.rect(0.1 * ppm, 0.09 * ppm, 0.37 * ppm, 0.23 * ppm);
    body.rect(0.37 * ppm, 0.1 * ppm, 0.385 * ppm, 0.23 * ppm);
    for (cx, cy) in [(0.112, 0.102), (0.372, 0.105)] {
        body.disc(p(cx, cy), 0.012 * ppm);
    }
    for k in 0..4 {
        let x = 0.115 + k as f32 * 0.058;
        glass.rect(x * ppm, 0.11 * ppm, (x + 0.048) * ppm, 0.16 * ppm);
    }
    glass.rect(0.352 * ppm, 0.11 * ppm, 0.378 * ppm, 0.18 * ppm);
    glass.rect(0.305 * ppm, 0.17 * ppm, 0.335 * ppm, 0.222 * ppm);
    glass.rect(0.1 * ppm, 0.232 * ppm, 0.385 * ppm, 0.26 * ppm);
    // Wheel arches cut out of the body, white tyres in them, blue hubs.
    let mut tyres = Canvas::new(w, h);
    let mut hubs = Canvas::new(w, h);
    for x in [0.16, 0.32] {
        glass.disc(p(x, 0.235), 0.034 * ppm);
        tyres.disc(p(x, 0.235), 0.025 * ppm);
        hubs.disc(p(x, 0.235), 0.009 * ppm);
    }
    ink.text_centered(
        "WEATHER",
        w as f32 * 0.5,
        0.4 * ppm,
        0.058 * ppm,
        0.011 * ppm,
    );
    ink.text_centered(
        "STATION",
        w as f32 * 0.5,
        0.48 * ppm,
        0.058 * ppm,
        0.011 * ppm,
    );
    for (i, label) in ["12", "34", "N7"].iter().enumerate() {
        let x0 = 0.05 + i as f32 * 0.133;
        routes[i].rect(x0 * ppm, 0.53 * ppm, (x0 + 0.115) * ppm, 0.62 * ppm);
        white_ink.text_centered(
            label,
            (x0 + 0.0575) * ppm,
            0.6 * ppm,
            0.05 * ppm,
            0.011 * ppm,
        );
    }
    let [r0, r1, r2] = &routes;
    compose(
        w,
        h,
        &[
            (&blue, hex(0x1d4f91)),
            (&body, [0.92, 0.93, 0.92]),
            (&glass, hex(0x1d4f91)),
            (&tyres, [0.92, 0.93, 0.92]),
            (&hubs, hex(0x1d4f91)),
            (&ink, hex(0x1b2430)),
            (r0, hex(0xc8302c)),
            (r1, hex(0x2f8a4c)),
            (r2, hex(0x2a2f6e)),
            (&white_ink, [0.93, 0.93, 0.92]),
        ],
        |u, v| {
            let g = sign_grime(u, v, 21);
            let k = 0.86 - 0.35 * g;
            [k, k, k * 0.98]
        },
    )
}

/// Timetable sheet (0.32 × 0.48 m): a header with the routes, three columns of departure
/// times (drawn as small figures) for weekdays, Saturday and Sunday.
pub fn timetable() -> Image {
    let (w, h) = (256u32, 384u32);
    let ppm = w as f32 / TIMETABLE_SIZE.0;
    let mut header = Canvas::new(w, h);
    let mut white = Canvas::new(w, h);
    let mut ink = Canvas::new(w, h);
    let mut grey = Canvas::new(w, h);
    header.rect(0.0, 0.0, w as f32, 0.065 * ppm);
    white.text(
        "12 34 N7",
        Vec2::new(0.015 * ppm, 0.045 * ppm),
        0.03 * ppm,
        0.006 * ppm,
        1.0,
    );
    for (i, day) in ["MON-FRI", "SAT", "SUN"].iter().enumerate() {
        let x = 0.015 + i as f32 * 0.102;
        ink.text(
            day,
            Vec2::new(x * ppm, 0.095 * ppm),
            0.014 * ppm,
            0.0032 * ppm,
            0.9,
        );
        grey.rect(x * ppm, 0.102 * ppm, (x + 0.09) * ppm, 0.104 * ppm);
        let rows = [22, 16, 12][i];
        for r in 0..rows {
            let y = 0.122 + r as f32 * 0.0155;
            let hour = 6 + r * 17 / rows;
            let mins = ["05", "25", "45"];
            let label = format!("{hour:02}:{}", mins[(r + i) % 3]);
            ink.text(
                &label,
                Vec2::new(x * ppm, y * ppm),
                0.0085 * ppm,
                0.0022 * ppm,
                0.85,
            );
        }
    }
    compose(
        w,
        h,
        &[
            (&header, hex(0x1d4f91)),
            (&white, [0.93, 0.93, 0.92]),
            (&ink, hex(0x1b2430)),
            (&grey, [0.5, 0.5, 0.5]),
        ],
        |u, v| {
            let k = 0.9 - 0.08 * sign_grime(u, v, 23);
            [k, k * 0.99, k * 0.95]
        },
    )
}

/// Face of the digital display (0.78 × 0.32 m): black, with a label and unit legends whose
/// indicator LEDs light for the quantity shown.
pub fn display_face() -> Image {
    let (w, h) = (640u32, 262u32);
    let ppm = w as f32 / 0.78;
    let mut white = Canvas::new(w, h);
    let mut grey = Canvas::new(w, h);
    white.text(
        "WEATHER NOW",
        Vec2::new(0.03 * ppm, 0.05 * ppm),
        0.022 * ppm,
        0.004 * ppm,
        1.0,
    );
    for (i, unit) in DISPLAY_UNITS.iter().enumerate() {
        let y = DISPLAY_UNIT_Y[i];
        white.text(
            unit,
            Vec2::new(0.6 * ppm, (0.16 - y + 0.012) * ppm),
            0.024 * ppm,
            0.0045 * ppm,
            1.0,
        );
    }
    let m = 0.012 * ppm;
    let (x1, y1) = (w as f32 - m, h as f32 - m);
    for (a, b) in [
        (Vec2::new(m, m), Vec2::new(x1, m)),
        (Vec2::new(x1, m), Vec2::new(x1, y1)),
        (Vec2::new(x1, y1), Vec2::new(m, y1)),
        (Vec2::new(m, y1), Vec2::new(m, m)),
    ] {
        grey.line(a, b, 0.003 * ppm);
    }
    compose(
        w,
        h,
        &[(&grey, [0.12, 0.12, 0.12]), (&white, [0.75, 0.75, 0.73])],
        |u, v| {
            let k = 0.012 + 0.006 * value(u * 40.0, v * 16.0, (40, 16), 3);
            [k, k, k]
        },
    )
}

/// Unit legends on the display, top to bottom.
pub const DISPLAY_UNITS: [&str; 3] = ["°C", "M/S", "MM/H"];
/// Height of each legend's indicator LED relative to the display center, meters.
pub const DISPLAY_UNIT_Y: [f32; 3] = [0.07, 0.0, -0.07];

/// "BUS STOP" road lettering (alpha = paint), with worn patches.
pub fn bus_stop_lettering() -> Image {
    let (w, h) = (1024u32, 256u32);
    let mut c = Canvas::new(w, h);
    let cap = 200.0;
    let tw = text_width("BUS STOP") * cap / 6.0 * 0.55;
    c.text(
        "BUS STOP",
        Vec2::new((w as f32 - tw) * 0.5, 228.0),
        cap,
        26.0,
        0.55,
    );
    let px = par_pixels(w, h, |x, y| {
        let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
        let wear = fbm(u * 8.0, v * 2.0, (8, 2), 5, 41) * 0.5 + 0.5;
        let grit = value(u * 400.0, v * 100.0, (400, 100), 42);
        let keep = if wear * 0.8 + grit * 0.35 > 0.78 {
            0.0
        } else {
            1.0
        };
        let a = c.at(x, y) * keep;
        let k = (0.9 - 0.15 * grit) * 255.0;
        [k as u8, k as u8, k as u8, (a * 255.0) as u8]
    });
    Image::new(w, h, px.concat(), true)
}
