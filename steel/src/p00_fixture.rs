//! P00's narrow controlled-world fixture command.
//!
//! This is enabled only for the local P00 development server. The recorder
//! still learns the resulting blocks through the Minecraft protocol.

use steel_core::command::{
    CommandArgument, CommandContext, CommandError, CommandNode, CommandRegistration,
    CommandRegistrationError, CommandRegistry, argument, literal,
};
use steel_registry::blocks::properties::{Direction, Half, SlabType, StairsShape};
use steel_registry::blocks::{block_state_ext::BlockStateExt, properties::BlockStateProperties};
use steel_registry::vanilla_blocks;
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Identifier, types::UpdateFlags};
use text_components::TextComponent;

const MIN_HORIZONTAL_POSITION: i32 = -29_999_999;
const MAX_ORIGIN_X: i32 = 29_999_996;
const MIN_BUILD_HEIGHT: i32 = -64;
const MAX_BUILD_HEIGHT: i32 = 319;

pub(super) fn register(registry: &mut CommandRegistry) -> Result<(), CommandRegistrationError> {
    registry.register(CommandRegistration::new(
        Identifier::from_steel("p00_fixture"),
        command,
    ))?;
    Ok(())
}

fn command() -> CommandNode {
    literal("p00_fixture").then(
        argument(
            "x",
            CommandArgument::integer(MIN_HORIZONTAL_POSITION, MAX_ORIGIN_X),
        )
        .then(
            argument(
                "y",
                CommandArgument::integer(MIN_BUILD_HEIGHT, MAX_BUILD_HEIGHT),
            )
            .then(
                argument(
                    "z",
                    CommandArgument::integer(MIN_HORIZONTAL_POSITION, -MIN_HORIZONTAL_POSITION),
                )
                .executes(place_fixture),
            ),
        ),
    )
}

fn place_fixture(context: &CommandContext<'_>) -> Result<i32, CommandError> {
    let x = required_integer(context, "x")?;
    let y = required_integer(context, "y")?;
    let z = required_integer(context, "z")?;
    let origin = BlockPos::new(x, y, z);
    let positions = [
        origin,
        origin.offset(1, 0, 0),
        origin.offset(2, 0, 0),
        origin.offset(3, 0, 0),
    ];

    let slab = vanilla_blocks::STONE_SLAB
        .default_state()
        .set_value(&BlockStateProperties::SLAB_TYPE, SlabType::Bottom);
    let stairs = vanilla_blocks::STONE_STAIRS
        .default_state()
        .set_value(&BlockStateProperties::FACING, Direction::North)
        .set_value(&BlockStateProperties::HALF, Half::Bottom)
        .set_value(&BlockStateProperties::STAIRS_SHAPE, StairsShape::Straight)
        .set_value(&BlockStateProperties::WATERLOGGED, false);
    let states = [
        vanilla_blocks::AIR.default_state(),
        vanilla_blocks::STONE.default_state(),
        slab,
        stairs,
    ];

    let world = context.source().world();
    for (position, state) in positions.into_iter().zip(states) {
        if !set_fixture_block(world, position, state) {
            return Err(CommandError::new(format!(
                "P00 fixture position {position:?} is unavailable"
            )));
        }
    }

    context.source().send_success(
        &TextComponent::plain(format!("P00 fixture placed at {origin:?}")),
        false,
    );
    Ok(states.len() as i32)
}

fn set_fixture_block(
    world: &std::sync::Arc<steel_core::world::World>,
    pos: BlockPos,
    state: BlockStateId,
) -> bool {
    let chunk_pos = ChunkPos::from_block_pos(pos);
    let Some(current_state) = world
        .chunk_map
        .with_full_chunk(chunk_pos, |chunk| chunk.get_block_state(pos))
    else {
        return false;
    };

    current_state == state || world.set_block(pos, state, UpdateFlags::UPDATE_ALL)
}

fn required_integer(context: &CommandContext<'_>, name: &str) -> Result<i32, CommandError> {
    context
        .integer(name)
        .ok_or_else(|| CommandError::new(format!("missing integer argument '{name}'")))
}
