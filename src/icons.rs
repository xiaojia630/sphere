//! Line icons drawn with the painter on a 24-unit grid, so they stay crisp at
//! any size and need no image assets.

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Agent,
    Code,
    Containers,
    Music,
    Mail,
    Calendar,
    Notes,
    Files,
    Web,
    Terminal,
    Monitor,
    Home,
    Min,
    Max,
    Close,
    Play,
    Pause,
    Next,
    Prev,
    Mic,
    Doc,
    Image,
}

pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width() / 24.0;
    let p = |x: f32, y: f32| Pos2::new(rect.left() + x * s, rect.top() + y * s);
    let stroke = Stroke::new((1.6 * s).max(1.0), color);
    let line = |pts: &[(f32, f32)]| {
        painter.add(Shape::line(
            pts.iter().map(|&(x, y)| p(x, y)).collect(),
            stroke,
        ));
    };
    let circle = |x: f32, y: f32, r: f32| {
        painter.circle_stroke(p(x, y), r * s, stroke);
    };
    let rect_at = |x: f32, y: f32, w: f32, h: f32| {
        line(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)]);
    };
    let fill = |pts: &[(f32, f32)]| {
        painter.add(Shape::convex_polygon(
            pts.iter().map(|&(x, y)| p(x, y)).collect(),
            color,
            Stroke::NONE,
        ));
    };

    match icon {
        Icon::Agent => {
            circle(12.0, 12.0, 3.0);
            for (a, b) in [
                ((12.0, 2.0), (12.0, 5.0)),
                ((12.0, 19.0), (12.0, 22.0)),
                ((2.0, 12.0), (5.0, 12.0)),
                ((19.0, 12.0), (22.0, 12.0)),
                ((5.0, 5.0), (7.0, 7.0)),
                ((17.0, 17.0), (19.0, 19.0)),
                ((5.0, 19.0), (7.0, 17.0)),
                ((17.0, 7.0), (19.0, 5.0)),
            ] {
                line(&[a, b]);
            }
        }
        Icon::Code => {
            line(&[(8.0, 7.0), (3.0, 12.0), (8.0, 17.0)]);
            line(&[(16.0, 7.0), (21.0, 12.0), (16.0, 17.0)]);
            line(&[(14.0, 4.0), (10.0, 20.0)]);
        }
        Icon::Containers => {
            line(&[
                (3.0, 8.0),
                (12.0, 3.0),
                (21.0, 8.0),
                (21.0, 16.0),
                (12.0, 21.0),
                (3.0, 16.0),
                (3.0, 8.0),
            ]);
            line(&[(3.0, 8.0), (12.0, 13.0), (21.0, 8.0)]);
            line(&[(12.0, 13.0), (12.0, 21.0)]);
        }
        Icon::Music => {
            circle(7.0, 17.0, 3.0);
            circle(17.0, 15.0, 3.0);
            line(&[(10.0, 17.0), (10.0, 5.0), (20.0, 3.0), (20.0, 15.0)]);
        }
        Icon::Mail => {
            rect_at(3.0, 5.0, 18.0, 14.0);
            line(&[(3.0, 7.0), (12.0, 13.0), (21.0, 7.0)]);
        }
        Icon::Calendar => {
            rect_at(3.0, 5.0, 18.0, 16.0);
            line(&[(3.0, 10.0), (21.0, 10.0)]);
            line(&[(8.0, 3.0), (8.0, 7.0)]);
            line(&[(16.0, 3.0), (16.0, 7.0)]);
        }
        Icon::Notes | Icon::Doc => {
            line(&[
                (6.0, 3.0),
                (15.0, 3.0),
                (19.0, 7.0),
                (19.0, 21.0),
                (6.0, 21.0),
                (6.0, 3.0),
            ]);
            if icon == Icon::Notes {
                line(&[(9.0, 11.0), (16.0, 11.0)]);
                line(&[(9.0, 15.0), (16.0, 15.0)]);
                line(&[(9.0, 7.0), (13.0, 7.0)]);
            }
        }
        Icon::Files => {
            line(&[
                (3.0, 6.0),
                (10.0, 6.0),
                (12.0, 8.0),
                (21.0, 8.0),
                (21.0, 19.0),
                (3.0, 19.0),
                (3.0, 6.0),
            ]);
        }
        Icon::Web => {
            circle(12.0, 12.0, 9.0);
            line(&[(3.0, 12.0), (21.0, 12.0)]);
            let ellipse: Vec<(f32, f32)> = (0..=24)
                .map(|i| {
                    let a = i as f32 / 24.0 * std::f32::consts::TAU;
                    (12.0 + 4.0 * a.cos(), 12.0 + 9.0 * a.sin())
                })
                .collect();
            line(&ellipse);
        }
        Icon::Terminal => {
            rect_at(3.0, 4.0, 18.0, 16.0);
            line(&[(7.0, 9.0), (10.0, 12.0), (7.0, 15.0)]);
            line(&[(12.0, 15.0), (17.0, 15.0)]);
        }
        Icon::Monitor => {
            line(&[
                (3.0, 12.0),
                (7.0, 12.0),
                (9.0, 6.0),
                (13.0, 18.0),
                (15.0, 12.0),
                (21.0, 12.0),
            ]);
        }
        Icon::Home => {
            circle(12.0, 12.0, 8.0);
            circle(12.0, 12.0, 3.0);
        }
        Icon::Min => line(&[(6.0, 12.0), (18.0, 12.0)]),
        Icon::Max => rect_at(6.0, 6.0, 12.0, 12.0),
        Icon::Close => {
            line(&[(7.0, 7.0), (17.0, 17.0)]);
            line(&[(17.0, 7.0), (7.0, 17.0)]);
        }
        Icon::Play => fill(&[(8.0, 5.0), (19.0, 12.0), (8.0, 19.0)]),
        Icon::Pause => {
            fill(&[(7.0, 5.0), (10.0, 5.0), (10.0, 19.0), (7.0, 19.0)]);
            fill(&[(14.0, 5.0), (17.0, 5.0), (17.0, 19.0), (14.0, 19.0)]);
        }
        Icon::Next => {
            fill(&[(6.0, 5.0), (15.0, 12.0), (6.0, 19.0)]);
            line(&[(18.0, 5.0), (18.0, 19.0)]);
        }
        Icon::Prev => {
            fill(&[(18.0, 5.0), (9.0, 12.0), (18.0, 19.0)]);
            line(&[(6.0, 5.0), (6.0, 19.0)]);
        }
        Icon::Mic => {
            rect_at(9.0, 3.0, 6.0, 11.0);
            let arc: Vec<(f32, f32)> = (0..=12)
                .map(|i| {
                    let a = i as f32 / 12.0 * std::f32::consts::PI;
                    (12.0 + 7.0 * a.cos(), 11.0 + 7.0 * a.sin())
                })
                .collect();
            line(&arc);
            line(&[(12.0, 18.0), (12.0, 21.0)]);
        }
        Icon::Image => {
            rect_at(3.0, 5.0, 18.0, 14.0);
            line(&[
                (3.0, 16.0),
                (8.0, 11.0),
                (12.0, 15.0),
                (15.0, 12.0),
                (21.0, 18.0),
            ]);
        }
    }
}
