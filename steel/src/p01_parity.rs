//! P01's opt-in scenario materializer for Steel/Azalea parity checks.
//!
//! The root project owns scenario generation. This command only consumes the
//! serialized materialization plan, applies ordinary 26.2 block states, and
//! moves the command-sending player to the requested test position.

use std::{fs, sync::Arc};

use rustc_hash::FxHashSet;
use steel_core::command::{
    CommandArgument, CommandContext, CommandError, CommandNode, CommandRegistration,
    CommandRegistrationError, CommandRegistry, argument, literal,
};
use steel_core::player::Player;
use steel_core::world::World;
use steel_registry::blocks::properties::{Direction, Half, SlabType, StairsShape};
use steel_registry::blocks::{block_state_ext::BlockStateExt, properties::BlockStateProperties};
use steel_registry::vanilla_blocks;
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Identifier, types::UpdateFlags};
use text_components::TextComponent;

const FORMAT_VERSION: u32 = 1;
const GENERATOR_VERSION: &str = "p01-physics-sandbox-v1";
const MIN_BUILD_HEIGHT: i32 = -64;
const MAX_BUILD_HEIGHT: i32 = 319;
const MIN_HORIZONTAL_POSITION: i32 = -29_999_999;
const MAX_HORIZONTAL_POSITION: i32 = 29_999_999;
const MAX_CLEAR_BLOCKS: u64 = 20_000;

pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(
        CommandRegistration::new(Identifier::from_steel("p01_materialize"), command)
            .default_access(),
    )?;
    registry.register(
        CommandRegistration::new(Identifier::from_steel("p01_clear"), clear_command)
            .default_access(),
    )?;
    Ok(())
}

fn command() -> CommandNode {
    literal("p01_materialize").then(
        argument("file", CommandArgument::string()).then(
            argument(
                "x",
                CommandArgument::double(
                    f64::from(MIN_HORIZONTAL_POSITION),
                    f64::from(MAX_HORIZONTAL_POSITION),
                ),
            )
            .then(
                argument(
                    "y",
                    CommandArgument::double(
                        f64::from(MIN_BUILD_HEIGHT),
                        f64::from(MAX_BUILD_HEIGHT),
                    ),
                )
                .then(
                    argument(
                        "z",
                        CommandArgument::double(
                            f64::from(MIN_HORIZONTAL_POSITION),
                            f64::from(MAX_HORIZONTAL_POSITION),
                        ),
                    )
                    .then(
                        argument("yaw", CommandArgument::float(-180.0, 180.0)).then(
                            argument("pitch", CommandArgument::float(-90.0, 90.0))
                                .executes(materialize_source)
                                .then(
                                    argument("target", CommandArgument::player())
                                        .executes(materialize_target),
                                ),
                        ),
                    ),
                ),
            ),
        ),
    )
}

fn clear_command() -> CommandNode {
    literal("p01_clear")
        .then(argument("file", CommandArgument::string()).executes(clear_materialization))
}

fn read_request(context: &CommandContext<'_>) -> Result<MaterializationRequest, CommandError> {
    let file = required_string(*context, "file")?;
    serde_json::from_str(
        &fs::read_to_string(file).map_err(|error| {
            CommandError::new(format!("cannot read P01 plan {file:?}: {error}"))
        })?,
    )
    .map_err(|error| CommandError::new(format!("cannot parse P01 plan {file:?}: {error}")))
}

fn clear_materialization(context: &CommandContext<'_>) -> Result<i32, CommandError> {
    let request = read_request(context)?;
    request.validate()?;
    let cleared_blocks = clear_region(
        context.source().world(),
        request.clear_min,
        request.clear_max,
    )?;
    context.source().send_success(
        &TextComponent::plain(format!(
            "P01 cleared {cleared_blocks} non-air blocks in {:?}..{:?}",
            request.clear_min, request.clear_max
        )),
        false,
    );
    log::info!(
        "P01 clear completed: cleared_non_air={} min={:?} max={:?}",
        cleared_blocks,
        request.clear_min,
        request.clear_max
    );
    i32::try_from(cleared_blocks)
        .map_err(|_| CommandError::new("P01 cleared block count does not fit command result"))
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "Steel's command callback API passes a borrowed context"
)]
fn materialize_source(context: &CommandContext<'_>) -> Result<i32, CommandError> {
    let player = context
        .source()
        .player()
        .cloned()
        .ok_or_else(|| CommandError::new("P01 materialization requires a player source"))?;
    materialize(context, player)
}

fn materialize_target(context: &CommandContext<'_>) -> Result<i32, CommandError> {
    let player = context.player("target")?;
    materialize(context, player)
}

fn materialize(context: &CommandContext<'_>, player: Arc<Player>) -> Result<i32, CommandError> {
    let target_x = required_double(*context, "x")?;
    let target_y = required_double(*context, "y")?;
    let target_z = required_double(*context, "z")?;
    let yaw = required_float(*context, "yaw")?;
    let pitch = required_float(*context, "pitch")?;
    let request = read_request(context)?;
    request.validate()?;

    let world = context.source().world();
    let cleared_blocks = clear_region(world, request.clear_min, request.clear_max)?;
    for block in &request.blocks {
        let position = BlockPos::new(block.position.x, block.position.y, block.position.z);
        if !set_block(world, position, block.block.state()) {
            return Err(CommandError::new(format!(
                "P01 block position {position:?} is unavailable"
            )));
        }
    }

    let mut target = context.source().position();
    target.x = target_x;
    target.y = target_y;
    target.z = target_z;
    player
        .teleport(target, yaw, pitch)
        .map_err(|error| CommandError::new(format!("cannot teleport P01 player: {error}")))?;

    context.source().send_success(
        &TextComponent::plain(format!(
            "P01 materialized {} blocks from seed {} after clearing {} non-air blocks for {}",
            request.blocks.len(),
            request.seed,
            cleared_blocks,
            player.gameprofile.name,
        )),
        false,
    );
    i32::try_from(request.blocks.len())
        .map_err(|_| CommandError::new("P01 block count does not fit command result"))
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
                        "P01 clear position {position:?} is unavailable"
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
    generator_version: String,
    seed: u64,
    clear_min: PlanPosition,
    clear_max: PlanPosition,
    blocks: Vec<MaterializationBlock>,
}

impl MaterializationRequest {
    fn validate(&self) -> Result<(), CommandError> {
        if self.format_version != FORMAT_VERSION {
            return Err(CommandError::new(format!(
                "unsupported P01 materialization version {}; expected {FORMAT_VERSION}",
                self.format_version
            )));
        }
        if self.generator_version != GENERATOR_VERSION {
            return Err(CommandError::new(format!(
                "unsupported P01 generator version {:?}; expected {GENERATOR_VERSION:?}",
                self.generator_version
            )));
        }
        validate_bounds(self.clear_min, self.clear_max)?;
        if region_volume(self.clear_min, self.clear_max)? > MAX_CLEAR_BLOCKS {
            return Err(CommandError::new(format!(
                "P01 clear region exceeds the {MAX_CLEAR_BLOCKS}-block development limit"
            )));
        }
        if self.blocks.is_empty() {
            return Err(CommandError::new(
                "P01 materialization plan contains no blocks",
            ));
        }

        let mut positions = FxHashSet::default();
        for block in &self.blocks {
            if !contains(self.clear_min, self.clear_max, block.position) {
                return Err(CommandError::new(format!(
                    "P01 block {:?} lies outside the clear region",
                    block.position
                )));
            }
            if !positions.insert(block.position) {
                return Err(CommandError::new(format!(
                    "P01 materialization plan repeats block position {:?}",
                    block.position
                )));
            }
        }
        Ok(())
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

#[derive(Debug, serde::Deserialize)]
struct MaterializationBlock {
    position: PlanPosition,
    block: MaterializationBlockKind,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum MaterializationBlockKind {
    Stone,
    BottomSlab,
    TopSlab,
    StraightStairsNorth,
}

impl MaterializationBlockKind {
    fn state(&self) -> BlockStateId {
        match self {
            Self::Stone => vanilla_blocks::STONE.default_state(),
            Self::BottomSlab => vanilla_blocks::STONE_SLAB
                .default_state()
                .set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Bottom),
            Self::TopSlab => vanilla_blocks::STONE_SLAB
                .default_state()
                .set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Top),
            Self::StraightStairsNorth => vanilla_blocks::STONE_STAIRS
                .default_state()
                .set_value(&BlockStateProperties::FACING, Direction::North)
                .set_value(&BlockStateProperties::HALF, Half::Bottom)
                .set_value(&BlockStateProperties::STAIRS_SHAPE, StairsShape::Straight)
                .set_value(&BlockStateProperties::WATERLOGGED, false),
        }
    }
}

fn validate_bounds(min: PlanPosition, max: PlanPosition) -> Result<(), CommandError> {
    if min.x > max.x || min.y > max.y || min.z > max.z {
        return Err(CommandError::new("P01 clear region has inverted bounds"));
    }
    if min.x < MIN_HORIZONTAL_POSITION
        || max.x > MAX_HORIZONTAL_POSITION
        || min.z < MIN_HORIZONTAL_POSITION
        || max.z > MAX_HORIZONTAL_POSITION
        || min.y < MIN_BUILD_HEIGHT
        || max.y > MAX_BUILD_HEIGHT
    {
        return Err(CommandError::new(
            "P01 clear region lies outside Minecraft bounds",
        ));
    }
    Ok(())
}

fn region_volume(min: PlanPosition, max: PlanPosition) -> Result<u64, CommandError> {
    let span = |low: i32, high: i32| {
        u64::try_from(i64::from(high) - i64::from(low) + 1)
            .map_err(|_| CommandError::new("P01 clear region span overflowed"))
    };
    let x = span(min.x, max.x)?;
    let y = span(min.y, max.y)?;
    let z = span(min.z, max.z)?;
    x.checked_mul(y)
        .and_then(|volume| volume.checked_mul(z))
        .ok_or_else(|| CommandError::new("P01 clear region volume overflowed"))
}

fn required_string<'context>(
    context: CommandContext<'context>,
    name: &str,
) -> Result<&'context str, CommandError> {
    context
        .string(name)
        .ok_or_else(|| CommandError::new(format!("missing string argument '{name}'")))
}

fn required_double(context: CommandContext<'_>, name: &str) -> Result<f64, CommandError> {
    context
        .double(name)
        .ok_or_else(|| CommandError::new(format!("missing double argument '{name}'")))
}

fn required_float(context: CommandContext<'_>, name: &str) -> Result<f32, CommandError> {
    context
        .float(name)
        .ok_or_else(|| CommandError::new(format!("missing float argument '{name}'")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(blocks: Vec<MaterializationBlock>) -> MaterializationRequest {
        MaterializationRequest {
            format_version: FORMAT_VERSION,
            generator_version: GENERATOR_VERSION.to_owned(),
            seed: 424_242,
            clear_min: PlanPosition { x: 0, y: 0, z: 0 },
            clear_max: PlanPosition { x: 1, y: 1, z: 1 },
            blocks,
        }
    }

    fn stone(position: PlanPosition) -> MaterializationBlock {
        MaterializationBlock {
            position,
            block: MaterializationBlockKind::Stone,
        }
    }

    #[test]
    fn accepts_a_valid_plan() {
        assert!(
            request(vec![stone(PlanPosition { x: 0, y: 0, z: 0 })])
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn rejects_duplicate_positions() {
        let position = PlanPosition { x: 0, y: 0, z: 0 };
        assert!(
            request(vec![stone(position), stone(position)])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn rejects_blocks_outside_the_clear_region() {
        assert!(
            request(vec![stone(PlanPosition { x: 2, y: 0, z: 0 })])
                .validate()
                .is_err()
        );
    }
}
