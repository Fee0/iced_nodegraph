//! Theme Nodes
//!
//! Two palette source nodes whose color pins can be wired straight into config
//! nodes' color inputs (or a ColorQuad builder) instead of hand-picking colors:
//!
//! - [`theme_node`] exposes the basic [`iced::theme::palette::Seed`] (the six flat
//!   entries).
//! - [`theme_extended_node`] exposes the
//!   [`palette`](iced::Theme::palette), each accent group as
//!   its `base`/`weak`/`strong` step.
//!
//! Both resolve from the current theme during propagation, so they follow theme
//! changes live.

use demo_common::NodeContentStyle;
use iced::Widget;
use iced::widget::{column, container, row, text};
use iced::{Alignment, Length};
use iced_nodegraph::pin;

use crate::nodes::{color_swatch, node_title_bar, pins};

/// One palette row: label on the left, the swatch carried by the output pin on
/// the right edge. The pin id is what edges and `theme_color` key on.
fn palette_row<'a, Message>(
    label: &'static str,
    id: &'static str,
    color: iced::Color,
) -> iced::Element<'a, Message>
where
    Message: Clone + 'a,
{
    row![
        text(label).size(10).width(Length::Fill),
        pin!(
            Right,
            id,
            color_swatch(Some(color)),
            Output,
            ::std::any::TypeId::of::<pins::ColorData>()
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .boxed()
}

/// Creates a Theme node exposing the basic [`iced::theme::palette::Seed`] as outputs.
pub fn theme_node<'a, Message>(theme: &'a iced::Theme) -> iced::Element<'a, Message>
where
    Message: Clone + 'a,
{
    use pins::theme as t;

    let style = NodeContentStyle::output(theme);
    let p = theme.seed();

    let content = column![
        palette_row("background", t::BACKGROUND, p.background),
        palette_row("text", t::TEXT, p.text),
        palette_row("primary", t::PRIMARY, p.primary),
        palette_row("success", t::SUCCESS, p.success),
        palette_row("warning", t::WARNING, p.warning),
        palette_row("danger", t::DANGER, p.danger),
    ]
    .spacing(4);

    column![
        node_title_bar("Theme", style),
        container(content).padding([8, 10])
    ]
    .width(160.0)
    .boxed()
}

/// Creates a Theme Extended node exposing the
/// [`palette`](iced::Theme::palette): each accent group as its
/// `base`/`weak`/`strong` step.
pub fn theme_extended_node<'a, Message>(theme: &'a iced::Theme) -> iced::Element<'a, Message>
where
    Message: Clone + 'a,
{
    use pins::theme_ext as t;

    let style = NodeContentStyle::output(theme);
    let p = theme.palette();

    let content = column![
        palette_row("bg base", t::BACKGROUND_BASE, p.background.base.color),
        palette_row("bg weak", t::BACKGROUND_WEAK, p.background.weak.color),
        palette_row("bg strong", t::BACKGROUND_STRONG, p.background.strong.color),
        palette_row("primary base", t::PRIMARY_BASE, p.primary.base.color),
        palette_row("primary weak", t::PRIMARY_WEAK, p.primary.weak.color),
        palette_row("primary strong", t::PRIMARY_STRONG, p.primary.strong.color),
        palette_row("secondary base", t::SECONDARY_BASE, p.secondary.base.color),
        palette_row("secondary weak", t::SECONDARY_WEAK, p.secondary.weak.color),
        palette_row(
            "secondary strong",
            t::SECONDARY_STRONG,
            p.secondary.strong.color
        ),
        palette_row("success base", t::SUCCESS_BASE, p.success.base.color),
        palette_row("success weak", t::SUCCESS_WEAK, p.success.weak.color),
        palette_row("success strong", t::SUCCESS_STRONG, p.success.strong.color),
        palette_row("warning base", t::WARNING_BASE, p.warning.base.color),
        palette_row("warning weak", t::WARNING_WEAK, p.warning.weak.color),
        palette_row("warning strong", t::WARNING_STRONG, p.warning.strong.color),
        palette_row("danger base", t::DANGER_BASE, p.danger.base.color),
        palette_row("danger weak", t::DANGER_WEAK, p.danger.weak.color),
        palette_row("danger strong", t::DANGER_STRONG, p.danger.strong.color),
    ]
    .spacing(4);

    column![
        node_title_bar("Theme Extended", style),
        container(content).padding([8, 10])
    ]
    .width(170.0)
    .boxed()
}
