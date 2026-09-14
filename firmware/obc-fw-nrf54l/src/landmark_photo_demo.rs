//! Display-only landmark photos through the production framebuffer and FLPR presenter.

use crate::ls021_flpr::{Frame64, Ls021Flpr, FB_H, FB_W};
use embassy_nrf::gpio::Input;
use embassy_time::Timer;
use embedded_graphics::{
    pixelcolor::{raw::RawU16, Rgb565},
    prelude::Point,
};
use obc_app::{assistant_demo::photos, screen::palette::*};
use obc_display::FbDevice64;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Surface,
};

pub async fn run(frame: &mut Frame64, panel: &mut Ls021Flpr<'_>, buttons: [Input<'static>; 4]) -> ! {
    let mut selected = 0;
    let mut page = 0;
    let mut large = true;
    let mut previous = [false; 4];
    let mut dirty = true;
    defmt::info!(
        "Landmark photos: Up/Down changes photo; Select changes size; Back opens/closes sources. SD untouched."
    );
    loop {
        if dirty {
            let (name, photo) = [
                ("Aare Gorge", &photos::AARE),
                ("Reichenbach Falls", &photos::FALLS),
                ("Dunlough Castle", &photos::DUNLOUGH),
            ][selected];
            {
                let mut fb = FbDevice64::new(frame.bytes_mut(), FB_W as u32, FB_H as u32);
                let color = |c| Rgb565::from(RawU16::new(c));
                let mut cv = Canvas::new(&mut fb, &color);
                cv.clear(PARCHMENT);
                cv.round(rect(4, 4, 232, 34), 6, WOOD);
                cv.text(name, Point::new(12, 9), Font::Label, TextAlign::Left, PARCHMENT);
                if page == 0 {
                    photos::draw(&mut cv, photo, if large { Point::new(12, 40) } else { Point::new(40, 86) }, large);
                    if !large {
                        cv.text("160 x 120", Point::new(120, 218), Font::Label, TextAlign::Center, INK);
                    }
                } else {
                    photos::paragraph(
                        &mut cv,
                        match page {
                            1 => photo.credit,
                            2 => photo.source,
                            _ => photo.licence,
                        },
                        66,
                    );
                }
                if page != 0 || !large {
                    cv.text("Up/down: photo", Point::new(120, 252), Font::Label, TextAlign::Center, SUBTEXT);
                }
                cv.round(rect(12, 280, 216, 32), 6, AMBER);
                cv.text(
                    if page == 0 {
                        if large {
                            "Smaller"
                        } else {
                            "Bigger"
                        }
                    } else {
                        "Next source"
                    },
                    Point::new(120, 282),
                    Font::Body,
                    TextAlign::Center,
                    INK,
                );
            }
            if !panel.push_frame(frame).await {
                defmt::warn!("Photo present stalled");
            }
            defmt::info!("Landmark photo: {=str}, page {=usize}, large {=bool}", name, page, large);
            dirty = false;
        }
        Timer::after_millis(30).await;
        let pressed = core::array::from_fn(|i| buttons[i].is_low());
        for i in 0..4 {
            if pressed[i] && !previous[i] {
                match i {
                    0 | 1 => {
                        selected = (selected + if i == 0 { 2 } else { 1 }) % 3;
                        page = 0;
                    }
                    2 => page = if page == 0 { 1 } else { 0 },
                    _ if page == 0 => large = !large,
                    _ => page = (page + 1) % 4,
                }
                dirty = true;
            }
        }
        previous = pressed;
    }
}
