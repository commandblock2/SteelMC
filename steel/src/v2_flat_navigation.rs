//! Opt-in V2 `FlatNavigation` reset command for live Steel qualification.
//!
//! The application writes a semantic request file. Steel owns registry
//! resolution, world mutation, and the normal player teleport/protocol path;
//! it does not regenerate the task.

use std::{fs, str::FromStr, sync::Arc};

use rustc_hash::FxHashSet;
use steel_core::command::{
    CommandArgument, CommandContext, CommandError, CommandNode, CommandRegistration,
    CommandRegistrationError, CommandRegistry, argument, literal,
};
use steel_core::{entity::Entity, world::World};
use steel_registry::{
    REGISTRY, RegistryExt, blocks::block_state_ext::BlockStateExt, vanilla_blocks,
};
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Identifier, types::UpdateFlags};
use text_components::TextComponent;

const FORMAT_VERSION: u32 = 1;
const MAX_CLEAR_BLOCKS: u64 = 20_000;
const MIN_BUILD_HEIGHT: i32 = -64;
const MAX_BUILD_HEIGHT: i32 = 319;
const MIN_HORIZONTAL_POSITION: i32 = -29_999_999;
const MAX_HORIZONTAL_POSITION: i32 = 29_999_999;

pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(
            Identifier::from_steel("v2_flat_navigation_materialize"),
            command,
        )
        .default_access(),
    )?;
    Ok(())
}

fn command() -> CommandNode {
    literal("v2_flat_navigation_materialize")
        .then(argument("file", CommandArgument::greedy_string()).executes(materialize))
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "Steel's command callback API passes a borrowed context"
)]
fn materialize(context: &CommandContext<'_>) -> Result<i32, CommandError> {
    let player = context
        .source()
        .player()
        .cloned()
        .ok_or_else(|| CommandError::new("V2 materialization requires a player source"))?;
    let file = required_string(*context, "file")?;
    let request = read_request(file)?;
    let blocks = request.validate_and_resolve()?;

    let world = context.source().world();
    let cleared_blocks = clear_region(world, request.clear_min, request.clear_max)?;
    for (position, state) in &blocks {
        if !set_block(world, *position, *state) {
            return Err(CommandError::new(format!(
                "V2 materialization position {position:?} is unavailable"
            )));
        }
    }

    let mut target = context.source().position();
    target.x = request.start.x;
    target.y = request.start.y;
    target.z = request.start.z;
    player
        .teleport(target, request.start.yaw, request.start.pitch)
        .map_err(|error| CommandError::new(format!("cannot teleport V2 player: {error}")))?;
    // The reset contract starts on the platform with no carried movement. The
    // regular teleport path clears velocity, while this explicit ground flag
    // also makes the server-side state deterministic before the next tick.
    player.set_on_ground(true);

    context.source().send_success(
        &TextComponent::plain(format!(
            "V2_FLAT_NAVIGATION_MATERIALIZED task_id={} blocks={} cleared={cleared_blocks}",
            request.task_id,
            blocks.len()
        )),
        false,
    );
    i32::try_from(blocks.len())
        .map_err(|_| CommandError::new("V2 block count does not fit command result"))
}

fn read_request(file: &str) -> Result<MaterializationRequest, CommandError> {
    serde_json::from_str(&fs::read_to_string(file).map_err(|error| {
        CommandError::new(format!(
            "cannot read V2 materialization plan {file:?}: {error}"
        ))
    })?)
    .map_err(|error| CommandError::new(format!("cannot parse V2 materialization plan: {error}")))
}

fn clear_region(
    world: &Arc<World>,
    min: PlanPosition,
    max: PlanPosition,
) -> Result<u64, CommandError> {
    let air = vanilla_blocks::AIR.default_state();
    let mut cleared_blocks = 0;
    for y in min.y..=max.y {
        for x in min.x..=max.x {
            for z in min.z..=max.z {
                let position = BlockPos::new(x, y, z);
                if !world.get_block_state(position).is_air() {
                    cleared_blocks += 1;
                }
                if !set_block(world, position, air) {
                    return Err(CommandError::new(format!(
                        "V2 clear position {position:?} is unavailable"
                    )));
                }
            }
        }
    }
    Ok(cleared_blocks)
}

fn set_block(world: &Arc<World>, position: BlockPos, state: BlockStateId) -> bool {
    let chunk_pos = ChunkPos::from_block_pos(position);
    let Some(current_state) = world
        .chunk_map
        .with_full_chunk(chunk_pos, |chunk| chunk.get_block_state(position))
    else {
        return false;
    };

    current_state == state || world.set_block(position, state, UpdateFlags::UPDATE_ALL)
}

#[derive(Debug, serde::Deserialize)]
struct MaterializationRequest {
    format_version: u32,
    task_id: String,
    clear_min: PlanPosition,
    clear_max: PlanPosition,
    start: PlanStart,
    blocks: Vec<MaterializationBlock>,
}

impl MaterializationRequest {
    fn validate_and_resolve(&self) -> Result<Vec<(BlockPos, BlockStateId)>, CommandError> {
        if self.format_version != FORMAT_VERSION {
            return Err(CommandError::new(format!(
                "unsupported V2 materialization version {}; expected {FORMAT_VERSION}",
                self.format_version
            )));
        }
        if self.task_id.is_empty() {
            return Err(CommandError::new("V2 materialization task id is empty"));
        }
        validate_bounds(self.clear_min, self.clear_max)?;
        if region_volume(self.clear_min, self.clear_max)? > MAX_CLEAR_BLOCKS {
            return Err(CommandError::new(format!(
                "V2 clear region exceeds the {MAX_CLEAR_BLOCKS}-block development limit"
            )));
        }
        validate_start(self.start)?;
        if self.blocks.is_empty() {
            return Err(CommandError::new(
                "V2 materialization plan contains no blocks",
            ));
        }

        let mut positions = FxHashSet::default();
        self.blocks
            .iter()
            .map(|block| {
                if !contains(self.clear_min, self.clear_max, block.position) {
                    return Err(CommandError::new(format!(
                        "V2 block {:?} lies outside the clear region",
                        block.position
                    )));
                }
                let position = BlockPos::new(block.position.x, block.position.y, block.position.z);
                if !positions.insert(position) {
                    return Err(CommandError::new(format!(
                        "V2 materialization plan repeats block position {position:?}"
                    )));
                }
                Ok((position, resolve_state(&block.state)?))
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, serde::Deserialize, PartialEq)]
struct PlanPosition {
    x: i32,
    y: i32,
    z: i32,
}

fn contains(min: PlanPosition, max: PlanPosition, position: PlanPosition) -> bool {
    (min.x..=max.x).contains(&position.x)
        && (min.y..=max.y).contains(&position.y)
        && (min.z..=max.z).contains(&position.z)
}

#[derive(Clone, Copy, Debug, serde::Deserialize)]
struct PlanStart {
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
}

fn validate_start(start: PlanStart) -> Result<(), CommandError> {
    if !start.x.is_finite()
        || !start.y.is_finite()
        || !start.z.is_finite()
        || !start.yaw.is_finite()
        || !start.pitch.is_finite()
    {
        return Err(CommandError::new(
            "V2 materialization start contains a non-finite value",
        ));
    }
    if !(-90.0..=90.0).contains(&start.pitch) {
        return Err(CommandError::new(
            "V2 materialization start pitch is outside -90..=90 degrees",
        ));
    }
    if start.x < f64::from(MIN_HORIZONTAL_POSITION)
        || start.x > f64::from(MAX_HORIZONTAL_POSITION)
        || start.z < f64::from(MIN_HORIZONTAL_POSITION)
        || start.z > f64::from(MAX_HORIZONTAL_POSITION)
        || start.y < f64::from(MIN_BUILD_HEIGHT)
        || start.y > f64::from(MAX_BUILD_HEIGHT + 1)
    {
        return Err(CommandError::new(
            "V2 materialization start lies outside Minecraft bounds",
        ));
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct MaterializationBlock {
    position: PlanPosition,
    state: String,
}

fn resolve_state(value: &str) -> Result<BlockStateId, CommandError> {
    let (name, encoded_properties) = match value.split_once('[') {
        Some((name, rest)) => {
            let properties = rest
                .strip_suffix(']')
                .ok_or_else(|| CommandError::new("V2 block state has an unmatched bracket"))?;
            if properties.is_empty() {
                return Err(CommandError::new(
                    "V2 block state property list must not be empty",
                ));
            }
            (name, Some(properties))
        }
        None if value.contains(']') => {
            return Err(CommandError::new(
                "V2 block state has an unmatched closing bracket",
            ));
        }
        None => (value, None),
    };
    let identifier = Identifier::from_str(name)
        .map_err(|_| CommandError::new(format!("invalid V2 block name {name:?}")))?;
    let block = REGISTRY
        .blocks
        .by_key(&identifier)
        .ok_or_else(|| CommandError::new(format!("unknown V2 block {name:?}")))?;
    let mut seen = FxHashSet::default();
    let properties = encoded_properties
        .into_iter()
        .flat_map(|encoded| encoded.split(','))
        .map(|property| {
            let (key, value) = property.split_once('=').ok_or_else(|| {
                CommandError::new(format!("invalid V2 block property {property:?}"))
            })?;
            if !seen.insert(key) {
                return Err(CommandError::new(format!(
                    "duplicate V2 block property {key:?}"
                )));
            }
            Ok((key, value))
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    REGISTRY
        .blocks
        .state_id_from_block_defaulted_properties(block, properties)
        .ok_or_else(|| CommandError::new(format!("invalid V2 block properties for {name:?}")))
}

fn validate_bounds(min: PlanPosition, max: PlanPosition) -> Result<(), CommandError> {
    if min.x > max.x || min.y > max.y || min.z > max.z {
        return Err(CommandError::new("V2 clear region has inverted bounds"));
    }
    if min.x < MIN_HORIZONTAL_POSITION
        || max.x > MAX_HORIZONTAL_POSITION
        || min.z < MIN_HORIZONTAL_POSITION
        || max.z > MAX_HORIZONTAL_POSITION
        || min.y < MIN_BUILD_HEIGHT
        || max.y > MAX_BUILD_HEIGHT
    {
        return Err(CommandError::new(
            "V2 clear region lies outside Minecraft bounds",
        ));
    }
    Ok(())
}

fn region_volume(min: PlanPosition, max: PlanPosition) -> Result<u64, CommandError> {
    let span = |low: i32, high: i32| {
        u64::try_from(i64::from(high) - i64::from(low) + 1)
            .map_err(|_| CommandError::new("V2 clear region span overflowed"))
    };
    let x = span(min.x, max.x)?;
    let y = span(min.y, max.y)?;
    let z = span(min.z, max.z)?;
    x.checked_mul(y)
        .and_then(|volume| volume.checked_mul(z))
        .ok_or_else(|| CommandError::new("V2 clear region volume overflowed"))
}

fn required_string<'context>(
    context: CommandContext<'context>,
    name: &str,
) -> Result<&'context str, CommandError> {
    context
        .string(name)
        .ok_or_else(|| CommandError::new(format!("missing string argument '{name}'")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use steel_registry::test_support::init_test_registry;

    fn request() -> MaterializationRequest {
        MaterializationRequest {
            format_version: FORMAT_VERSION,
            task_id: "v2/test".to_owned(),
            clear_min: PlanPosition { x: 0, y: 0, z: 0 },
            clear_max: PlanPosition { x: 1, y: 1, z: 1 },
            start: PlanStart {
                x: 0.5,
                y: 1.0,
                z: 0.5,
                yaw: 0.0,
                pitch: 0.0,
            },
            blocks: vec![MaterializationBlock {
                position: PlanPosition { x: 0, y: 0, z: 0 },
                state: "minecraft:stone".to_owned(),
            }],
        }
    }

    #[test]
    fn rejects_duplicate_positions_before_world_mutation() {
        init_test_registry();
        let mut request = request();
        request.blocks.push(MaterializationBlock {
            position: PlanPosition { x: 0, y: 0, z: 0 },
            state: "minecraft:stone".to_owned(),
        });
        assert!(request.validate_and_resolve().is_err());
    }

    #[test]
    fn rejects_clear_regions_that_exceed_the_development_limit() {
        let mut request = request();
        request.clear_max = PlanPosition {
            x: 100,
            y: 100,
            z: 100,
        };
        assert!(request.validate_and_resolve().is_err());
    }
}
