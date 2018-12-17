use crate::menu::{Menu, Position};
use crate::text::LINE_HEIGHT;
use crate::{Canvas, Color, GfxCtx, InputResult, Key, Text, UserInput};
use geom::{Polygon, Pt2D};
use std::collections::HashSet;

pub struct TopMenu {
    folders: Vec<Folder>,

    txt: Text,

    highlighted: Option<usize>,
    submenu: Option<(usize, Menu<Key>)>,
}

impl TopMenu {
    pub fn new(mut folders: Vec<Folder>, canvas: &Canvas) -> TopMenu {
        let mut keys: HashSet<Key> = HashSet::new();
        for f in &folders {
            for (key, _) in &f.actions {
                if keys.contains(key) {
                    panic!("TopMenu uses {:?} twice", key);
                }
                keys.insert(*key);
            }
        }

        let mut txt = Text::with_bg_color(None);
        for f in &folders {
            txt.append(format!("{}     ", f.name), Color::WHITE, None);
        }

        // Calculate rectangles for the folders
        {
            let mut x1 = 0.0;
            for f in folders.iter_mut() {
                let (w, h) = canvas.text_dims(&Text::from_line(f.name.to_string()));
                f.rectangle.x1 = x1;
                f.rectangle.x2 = x1 + w;
                f.rectangle.y2 = h;
                x1 += w;

                x1 += canvas.text_dims(&Text::from_line("     ".to_string())).0;
            }
        }

        TopMenu {
            folders,
            txt,
            highlighted: None,
            submenu: None,
        }
    }

    pub fn event(&mut self, input: &mut UserInput, canvas: &Canvas) {
        if let Some(cursor) = input.get_moved_mouse() {
            // TODO Could quickly filter out by checking y
            self.highlighted = self
                .folders
                .iter()
                .position(|f| f.rectangle.contains(cursor));
        }

        if let Some(idx) = self.highlighted {
            if input.left_mouse_button_pressed()
                || self
                    .submenu
                    .as_ref()
                    .map(|(existing_idx, _)| idx != *existing_idx)
                    .unwrap_or(false)
            {
                let f = &self.folders[idx];
                self.submenu = Some((
                    idx,
                    Menu::new(
                        None,
                        f.actions
                            .iter()
                            .map(|(key, action)| (Some(*key), action.to_string(), *key))
                            .collect(),
                        false,
                        Position::TopLeft(canvas.screen_to_map((f.rectangle.x1, f.rectangle.y2))),
                        canvas,
                    ),
                ));
                return;
            }
        }

        if let Some((_, ref mut submenu)) = self.submenu {
            if let Some(ev) = input.use_event_directly() {
                match submenu.event(ev, canvas) {
                    InputResult::StillActive => {}
                    InputResult::Canceled => {
                        self.submenu = None;
                        self.highlighted = None;
                    }
                    InputResult::Done(_, key) => {
                        println!("submenu finished with {:?}", key);
                        self.submenu = None;
                        self.highlighted = None;
                    }
                };
            }
        }
    }

    pub fn draw(&self, g: &mut GfxCtx, canvas: &Canvas) {
        let old_ctx = g.fork_screenspace();
        g.draw_polygon(
            Color::BLACK.alpha(0.5),
            &Polygon::rectangle_topleft(
                Pt2D::new(0.0, 0.0),
                canvas.window_size.width as f64,
                LINE_HEIGHT,
            ),
        );

        if let Some(idx) = self.highlighted {
            let r = &self.folders[idx].rectangle;
            g.draw_polygon(
                Color::RED,
                &Polygon::rectangle_topleft(Pt2D::new(r.x1, r.y1), r.x2 - r.x1, r.y2 - r.y1),
            );
        }
        g.unfork(old_ctx);

        canvas.draw_text_at_screenspace_topleft(g, self.txt.clone(), (0.0, 0.0));

        if let Some((_, ref menu)) = self.submenu {
            menu.draw(g, canvas);
        }
    }
}

pub struct Folder {
    name: String,
    actions: Vec<(Key, String)>,

    rectangle: Rectangle,
}

impl Folder {
    pub fn new(name: &str, actions: Vec<(Key, &str)>) -> Folder {
        Folder {
            name: name.to_string(),
            actions: actions
                .into_iter()
                .map(|(key, action)| (key, action.to_string()))
                .collect(),
            // TopMenu::new will calculate this.
            rectangle: Rectangle {
                x1: 0.0,
                y1: 0.0,
                x2: 0.0,
                y2: 0.0,
            },
        }
    }
}

// Everything in good ol' screenspace
struct Rectangle {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

impl Rectangle {
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x1 && x <= self.x2 && y >= self.y1 && y <= self.y2
    }
}
