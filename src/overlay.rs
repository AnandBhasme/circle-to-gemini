// Overlay module: fullscreen screenshot canvas with drawing support

use iced::mouse;
use iced::widget::canvas::{self, Event, Frame, Geometry, Path, Stroke};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme};

/// The drawing state for the selection overlay
#[derive(Debug, Clone)]
pub struct OverlayState {
    /// All points of the freehand path (or just start/end for rectangle)
    pub points: Vec<Point>,
    /// Whether user is currently drawing
    pub is_drawing: bool,
    /// Whether selection is complete
    pub selection_complete: bool,
    /// Use rectangle mode (shift held or default)
    pub rect_mode: bool,
    /// Start point for rectangle
    pub rect_start: Option<Point>,
    /// Current mouse position for rectangle preview
    pub rect_current: Option<Point>,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            is_drawing: false,
            selection_complete: false,
            rect_mode: true, // default to rectangle for reliability
            rect_start: None,
            rect_current: None,
        }
    }
}

impl OverlayState {
    pub fn reset(&mut self) {
        self.points.clear();
        self.is_drawing = false;
        self.selection_complete = false;
        self.rect_start = None;
        self.rect_current = None;
    }

    /// Get the bounding rectangle of the selection
    pub fn selection_bounds(&self) -> Option<SelectionRect> {
        if self.rect_mode {
            if let (Some(start), Some(end)) = (self.rect_start, self.rect_current) {
                let x = start.x.min(end.x);
                let y = start.y.min(end.y);
                let w = (start.x - end.x).abs();
                let h = (start.y - end.y).abs();
                if w > 10.0 && h > 10.0 {
                    return Some(SelectionRect {
                        x: x as u32,
                        y: y as u32,
                        width: w as u32,
                        height: h as u32,
                    });
                }
            }
        } else if self.points.len() > 2 {
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;
            for p in &self.points {
                min_x = min_x.min(p.x);
                min_y = min_y.min(p.y);
                max_x = max_x.max(p.x);
                max_y = max_y.max(p.y);
            }
            let w = max_x - min_x;
            let h = max_y - min_y;
            if w > 10.0 && h > 10.0 {
                return Some(SelectionRect {
                    x: min_x as u32,
                    y: min_y as u32,
                    width: w as u32,
                    height: h as u32,
                });
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SelectionRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Messages from the overlay canvas
#[derive(Debug, Clone)]
pub enum OverlayMessage {
    StartDraw(Point),
    Drawing(Point),
    EndDraw(Point),
    Cancel,
}

/// The canvas program for drawing selections on the screenshot
pub struct OverlayCanvas<'a> {
    pub state: &'a OverlayState,
}

impl<'a> canvas::Program<OverlayMessage> for OverlayCanvas<'a> {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<OverlayMessage>) {
        let Some(pos) = cursor.position_in(bounds) else {
            return (canvas::event::Status::Ignored, None);
        };

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                (
                    canvas::event::Status::Captured,
                    Some(OverlayMessage::StartDraw(pos)),
                )
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if self.state.is_drawing {
                    (
                        canvas::event::Status::Captured,
                        Some(OverlayMessage::Drawing(pos)),
                    )
                } else {
                    (canvas::event::Status::Ignored, None)
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if self.state.is_drawing {
                    (
                        canvas::event::Status::Captured,
                        Some(OverlayMessage::EndDraw(pos)),
                    )
                } else {
                    (canvas::event::Status::Ignored, None)
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                (
                    canvas::event::Status::Captured,
                    Some(OverlayMessage::Cancel),
                )
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            }) => {
                (
                    canvas::event::Status::Captured,
                    Some(OverlayMessage::Cancel),
                )
            }
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());

        // Dim the screen with a subtle semi-transparent dark overlay
        frame.fill_rectangle(
            Point::ORIGIN,
            bounds.size(),
            Color::from_rgba(0.0, 0.0, 0.0, 0.25),
        );

        // Draw the selection
        if self.state.rect_mode {
            // Rectangle selection
            if let (Some(start), Some(current)) = (self.state.rect_start, self.state.rect_current) {
                let x = start.x.min(current.x);
                let y = start.y.min(current.y);
                let w = (start.x - current.x).abs();
                let h = (start.y - current.y).abs();

                if w > 2.0 && h > 2.0 {
                    // Clear the selected area (make it bright/visible)
                    frame.fill_rectangle(
                        Point::new(x, y),
                        Size::new(w, h),
                        Color::from_rgba(0.0, 0.0, 0.0, 0.0),
                    );

                    // Outer glow (Gemini blue-purple gradient effect)
                    let glow_colors = [
                        Color::from_rgba(0.35, 0.53, 0.98, 0.15),  // blue
                        Color::from_rgba(0.60, 0.40, 0.90, 0.12),  // purple
                        Color::from_rgba(0.35, 0.53, 0.98, 0.08),  // blue faint
                    ];

                    for (i, color) in glow_colors.iter().enumerate() {
                        let offset = (i as f32 + 1.0) * 3.0;
                        let glow_rect = Path::rectangle(
                            Point::new(x - offset, y - offset),
                            Size::new(w + offset * 2.0, h + offset * 2.0),
                        );
                        frame.stroke(
                            &glow_rect,
                            Stroke::default()
                                .with_color(*color)
                                .with_width(3.0),
                        );
                    }

                    // Main selection border
                    let rect_path = Path::rectangle(
                        Point::new(x, y),
                        Size::new(w, h),
                    );
                    frame.stroke(
                        &rect_path,
                        Stroke::default()
                            .with_color(Color::from_rgb(0.35, 0.53, 0.98))
                            .with_width(2.5),
                    );

                    // Corner handles
                    let handle_size = 8.0;
                    let corners = [
                        Point::new(x, y),
                        Point::new(x + w, y),
                        Point::new(x, y + h),
                        Point::new(x + w, y + h),
                    ];
                    for corner in corners {
                        frame.fill_rectangle(
                            Point::new(corner.x - handle_size / 2.0, corner.y - handle_size / 2.0),
                            Size::new(handle_size, handle_size),
                            Color::from_rgb(0.35, 0.53, 0.98),
                        );
                    }
                }
            }
        } else if self.state.points.len() > 1 {
            // Freehand lasso
            let path = Path::new(|builder| {
                builder.move_to(self.state.points[0]);
                for p in &self.state.points[1..] {
                    builder.line_to(*p);
                }
                if self.state.selection_complete {
                    builder.close();
                }
            });

            // Glow effect
            for (i, alpha) in [0.15f32, 0.10, 0.06].iter().enumerate() {
                let width = 6.0 + (i as f32) * 4.0;
                frame.stroke(
                    &path,
                    Stroke::default()
                        .with_color(Color::from_rgba(0.35, 0.53, 0.98, *alpha))
                        .with_width(width),
                );
            }

            // Main stroke
            frame.stroke(
                &path,
                Stroke::default()
                    .with_color(Color::from_rgb(0.35, 0.53, 0.98))
                    .with_width(2.5),
            );
        }

        // Instruction text at top center
        if !self.state.is_drawing && !self.state.selection_complete {
            // We can't easily draw text on canvas, so this is handled by the overlay view
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}
