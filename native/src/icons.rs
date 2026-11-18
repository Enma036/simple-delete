//! Original, resolution-independent line icons drawn directly by egui.
use eframe::egui::{self, Color32, Painter, Rect, Stroke};

#[derive(Clone, Copy)]
pub enum Icon {
    Apps,
    Large,
    Recent,
    Store,
    Suggestions,
    Cleanup,
    Startup,
    Recovery,
    Activity,
    Hardware,
    Shared,
    Security,
    Windows,
    Games,
    Developer,
    Creative,
    Browser,
    Productivity,
    Communication,
    Other,
}

impl Icon {
    pub fn category(category: &str) -> Self {
        match category {
            "Drivers & hardware" => Self::Hardware,
            "Shared components" => Self::Shared,
            "Security & networking" => Self::Security,
            "Windows components" => Self::Windows,
            "Games & launchers" => Self::Games,
            "Developer tools" => Self::Developer,
            "Creative tools" => Self::Creative,
            "Browsers" => Self::Browser,
            "Productivity" => Self::Productivity,
            "Communication" => Self::Communication,
            _ => Self::Other,
        }
    }
    pub fn page(page: &str) -> Self {
        match page {
            "All apps" => Self::Apps,
            "Large apps" => Self::Large,
            "Recently installed" => Self::Recent,
            "Store apps" => Self::Store,
            "Suggestions" => Self::Suggestions,
            "Cleanup" | crate::complete_cleanup::PAGE => Self::Cleanup,
            "Startup" => Self::Startup,
            "Recovery" => Self::Recovery,
            _ => Self::Activity,
        }
    }
    pub fn colors(self) -> (Color32, Color32) {
        let (bg, fg) = match self {
            Self::Hardware => ([226, 237, 248], [62, 105, 151]),
            Self::Shared | Self::Developer => ([235, 229, 252], [114, 76, 180]),
            Self::Security | Self::Recovery => ([221, 243, 234], [30, 130, 94]),
            Self::Games | Self::Suggestions => ([251, 231, 244], [176, 65, 134]),
            Self::Creative | Self::Cleanup => ([255, 237, 219], [185, 107, 33]),
            Self::Browser | Self::Windows | Self::Apps => ([225, 235, 255], [48, 106, 206]),
            Self::Communication => ([221, 242, 245], [27, 126, 144]),
            Self::Productivity | Self::Activity => ([249, 238, 207], [156, 119, 25]),
            _ => ([234, 238, 245], [90, 109, 138]),
        };
        (
            Color32::from_rgb(bg[0], bg[1], bg[2]),
            Color32::from_rgb(fg[0], fg[1], fg[2]),
        )
    }
}

pub fn badge(ui: &mut egui::Ui, icon: Icon, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    badge_at(ui.painter(), rect, icon);
}

pub fn badge_at(p: &Painter, rect: Rect, icon: Icon) {
    let (bg, fg) = icon.colors();
    p.rect_filled(rect, rect.width() * 0.25, bg);
    paint(p, rect.shrink(rect.width() * 0.21), icon, fg);
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let scale = rect.width().min(rect.height()) / 24.0;
    let origin = rect.center() - egui::vec2(12.0, 12.0) * scale;
    let pt = |x: f32, y: f32| origin + egui::vec2(x, y) * scale;
    let stroke = Stroke::new((1.7 * scale).max(1.0), color);
    let line = |points: &[(f32, f32)]| {
        p.add(egui::Shape::line(
            points.iter().map(|&(x, y)| pt(x, y)).collect(),
            stroke,
        ));
    };
    let box_at = |x: f32, y: f32, w: f32, h: f32, r: f32| {
        p.rect_stroke(
            Rect::from_min_size(pt(x, y), egui::vec2(w, h) * scale),
            r * scale,
            stroke,
            egui::StrokeKind::Inside,
        );
    };
    let circle = |x: f32, y: f32, r: f32| {
        p.circle_stroke(pt(x, y), r * scale, stroke);
    };
    match icon {
        Icon::Apps => {
            for (x, y) in [(3., 3.), (14., 3.), (3., 14.), (14., 14.)] {
                box_at(x, y, 7., 7., 1.5);
            }
        }
        Icon::Large => {
            box_at(3., 4., 18., 16., 3.);
            line(&[(3., 14.), (21., 14.)]);
            circle(17., 17., 0.7);
            line(&[(7., 8.), (7., 11.), (10., 11.)]);
            line(&[(17., 8.), (17., 11.), (14., 11.)]);
        }
        Icon::Recent => {
            circle(11., 11., 8.);
            line(&[(11., 6.), (11., 11.), (7., 13.)]);
            line(&[(17., 17.), (23., 17.)]);
            line(&[(20., 14.), (20., 20.)]);
        }
        Icon::Store => {
            line(&[(4., 8.), (20., 8.), (21., 21.), (3., 21.), (4., 8.)]);
            line(&[
                (8., 9.),
                (8., 6.),
                (9., 3.),
                (15., 3.),
                (16., 6.),
                (16., 9.),
            ]);
        }
        Icon::Suggestions => {
            line(&[
                (12., 2.),
                (14.5, 9.5),
                (22., 12.),
                (14.5, 14.5),
                (12., 22.),
                (9.5, 14.5),
                (2., 12.),
                (9.5, 9.5),
                (12., 2.),
            ]);
            line(&[(19., 2.), (19., 6.)]);
            line(&[(17., 4.), (21., 4.)]);
        }
        Icon::Cleanup => {
            line(&[(15., 3.), (8., 12.)]);
            line(&[(6., 11.), (14., 16.), (11., 22.), (2., 17.), (6., 11.)]);
            line(&[(7., 15.), (5., 19.)]);
            line(&[(10., 17.), (8., 21.)]);
            line(&[(17., 13.), (22., 13.)]);
            line(&[(19., 17.), (22., 17.)]);
        }
        Icon::Startup => {
            line(&[(12., 2.), (12., 11.)]);
            line(&[
                (6., 5.),
                (3., 9.),
                (3., 15.),
                (6., 20.),
                (12., 22.),
                (18., 20.),
                (21., 15.),
                (21., 9.),
                (18., 5.),
            ]);
        }
        Icon::Recovery => {
            line(&[
                (3., 9.),
                (6., 5.),
                (12., 3.),
                (18., 5.),
                (21., 10.),
                (20., 16.),
                (15., 21.),
                (8., 21.),
                (4., 18.),
            ]);
            line(&[(3., 3.), (3., 9.), (9., 9.)]);
            line(&[(12., 7.), (12., 12.), (16., 14.)]);
        }
        Icon::Activity => {
            for y in [5., 12., 19.] {
                circle(3., y, 1.);
                line(&[(8., y), (21., y)]);
            }
        }
        Icon::Hardware => {
            box_at(6., 6., 12., 12., 2.);
            box_at(9., 9., 6., 6., 1.);
            for x in [8., 12., 16.] {
                line(&[(x, 2.), (x, 6.)]);
                line(&[(x, 18.), (x, 22.)]);
                line(&[(2., x), (6., x)]);
                line(&[(18., x), (22., x)]);
            }
        }
        Icon::Shared => {
            line(&[(12., 2.), (22., 7.), (12., 12.), (2., 7.), (12., 2.)]);
            line(&[(2., 12.), (12., 17.), (22., 12.)]);
            line(&[(2., 17.), (12., 22.), (22., 17.)]);
        }
        Icon::Security => {
            line(&[
                (12., 2.),
                (21., 6.),
                (20., 14.),
                (17., 19.),
                (12., 22.),
                (7., 19.),
                (4., 14.),
                (3., 6.),
                (12., 2.),
            ]);
            line(&[(8., 12.), (11., 15.), (17., 9.)]);
        }
        Icon::Windows => {
            for (x, y) in [(3., 3.), (13., 3.), (3., 13.), (13., 13.)] {
                p.rect_filled(
                    Rect::from_min_size(pt(x, y), egui::vec2(8., 8.) * scale),
                    scale,
                    color,
                );
            }
        }
        Icon::Games => {
            line(&[
                (7., 7.),
                (17., 7.),
                (20., 10.),
                (22., 18.),
                (20., 20.),
                (16., 17.),
                (8., 17.),
                (4., 20.),
                (2., 18.),
                (4., 10.),
                (7., 7.),
            ]);
            line(&[(6., 10.), (6., 14.)]);
            line(&[(4., 12.), (8., 12.)]);
            circle(16., 11., 0.8);
            circle(19., 14., 0.8);
        }
        Icon::Developer => {
            line(&[(8., 5.), (2., 12.), (8., 19.)]);
            line(&[(16., 5.), (22., 12.), (16., 19.)]);
            line(&[(14., 3.), (10., 21.)]);
        }
        Icon::Creative => {
            line(&[
                (5., 16.),
                (16., 3.),
                (21., 8.),
                (8., 21.),
                (3., 22.),
                (5., 16.),
            ]);
            line(&[(13., 6.), (18., 11.)]);
            line(&[(5., 16.), (8., 17.), (8., 21.)]);
        }
        Icon::Browser => {
            circle(12., 12., 10.);
            let points = (0..=32)
                .map(|i| {
                    let angle = i as f32 * std::f32::consts::TAU / 32.0;
                    pt(12.0 + angle.cos() * 5.0, 12.0 + angle.sin() * 10.0)
                })
                .collect();
            p.add(egui::Shape::line(points, stroke));
            line(&[(2., 12.), (22., 12.)]);
            line(&[(4., 6.), (20., 6.)]);
            line(&[(4., 18.), (20., 18.)]);
        }
        Icon::Productivity => {
            line(&[
                (5., 2.),
                (14., 2.),
                (20., 8.),
                (20., 22.),
                (5., 22.),
                (5., 2.),
            ]);
            line(&[(14., 2.), (14., 8.), (20., 8.)]);
            line(&[(8., 12.), (16., 12.)]);
            line(&[(8., 16.), (16., 16.)]);
        }
        Icon::Communication => {
            line(&[
                (3., 3.),
                (18., 3.),
                (18., 14.),
                (9., 14.),
                (4., 18.),
                (4., 14.),
                (3., 14.),
                (3., 3.),
            ]);
            line(&[
                (21., 8.),
                (22., 8.),
                (22., 20.),
                (19., 20.),
                (19., 23.),
                (15., 20.),
                (11., 20.),
            ]);
            for x in [7., 11., 15.] {
                circle(x, 8., 0.6);
            }
        }
        Icon::Other => {
            line(&[
                (12., 2.),
                (22., 7.),
                (22., 17.),
                (12., 22.),
                (2., 17.),
                (2., 7.),
                (12., 2.),
            ]);
            line(&[(2., 7.), (12., 12.), (22., 7.)]);
            line(&[(12., 12.), (12., 22.)]);
            line(&[(7., 4.5), (17., 9.5)]);
        }
    }
}
