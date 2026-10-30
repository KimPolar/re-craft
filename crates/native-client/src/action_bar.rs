//! Transient, tick-driven overlay for a locked chest.
use crate::{menu_assets::MenuAssets, menu_draw::Canvas};
use native_persistence::nbt::Tag;
use native_session::LockedChestMessage;

pub fn display_key(message: Option<&LockedChestMessage>, tick: u64) -> Option<(u64, u8)> {
    let message = message?;
    let remaining = 60_u64.checked_sub(tick.saturating_sub(message.tick))?;
    if remaining == 0 {
        return None;
    }
    Some((
        message.sequence,
        (remaining.saturating_mul(255) / 20).min(255) as u8,
    ))
}

pub fn draw(
    canvas: &mut Canvas,
    assets: Option<&MenuAssets>,
    message: Option<&LockedChestMessage>,
    tick: u64,
) {
    let Some((_, opacity)) = display_key(message, tick) else {
        return;
    };
    let message = message.unwrap();
    let title = message.custom_name.clone().unwrap_or_else(|| {
        Tag::Compound(vec![(
            "translate".into(),
            Tag::String(
                if message.double_chest {
                    "container.chestDouble"
                } else {
                    "container.chest"
                }
                .into(),
            ),
        )])
    });
    let component = Tag::Compound(vec![
        ("translate".into(), Tag::String("container.isLocked".into())),
        ("fallback".into(), Tag::String("%s is locked!".into())),
        (
            "with".into(),
            Tag::List {
                element_type: title.tag_type(),
                values: vec![title],
            },
        ),
    ]);
    let Some(spans) = crate::text_component::resolve(&component, |key, fallback| {
        assets.map_or_else(
            || match key {
                "container.chest" => "Chest".into(),
                "container.chestDouble" => "Large Chest".into(),
                _ => fallback.into(),
            },
            |assets| assets.translate(key, fallback).to_owned(),
        )
    }) else {
        return;
    };
    let width = canvas.component_width(assets, &spans);
    canvas.component_shadow_alpha(
        assets,
        canvas.width / 2 - width / 2,
        canvas.height - 72,
        &spans,
        opacity,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_lasts_sixty_ticks_and_fades_for_the_last_twenty() {
        let message = LockedChestMessage {
            tick: 10,
            sequence: 3,
            double_chest: false,
            custom_name: None,
        };
        assert_eq!(display_key(Some(&message), 10), Some((3, 255)));
        assert_eq!(display_key(Some(&message), 50), Some((3, 255)));
        assert_eq!(display_key(Some(&message), 51), Some((3, 242)));
        assert_eq!(display_key(Some(&message), 69), Some((3, 12)));
        assert_eq!(display_key(Some(&message), 70), None);
        let mut canvas = Canvas::new(320, 240);
        draw(&mut canvas, None, Some(&message), 10);
        assert!(canvas.pixels.iter().any(|&pixel| pixel != 0));
        let mut expired = Canvas::new(320, 240);
        draw(&mut expired, None, Some(&message), 70);
        assert!(expired.pixels.iter().all(|&pixel| pixel == 0));
        let mut named = message.clone();
        named.custom_name = Some(Tag::Compound(vec![
            ("text".into(), Tag::String("Vault".into())),
            ("color".into(), Tag::String("red".into())),
        ]));
        let mut colored = Canvas::new(320, 240);
        draw(&mut colored, None, Some(&named), 10);
        assert!(colored.pixels.contains(&0xffff5555));
        assert_ne!(colored.pixels, canvas.pixels);
    }
}
