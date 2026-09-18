//! Conversion between the v2 semantic block-state descriptor and Steel state IDs.

use std::{collections::BTreeMap, error::Error, fmt};

use parkour_bot_azalea::BlockStateDescriptor;
use steel_utils::{BlockStateId, Identifier};

use crate::{RegistryExt, REGISTRY};

/// Errors raised while resolving a semantic descriptor through Steel's registry.
#[derive(Debug, Eq, PartialEq)]
pub enum BlockStateDescriptorError {
    InvalidBlockName(String),
    UnknownBlock(String),
    InvalidProperties(String),
    UnknownState(u16),
}

impl fmt::Display for BlockStateDescriptorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBlockName(name) => write!(formatter, "invalid Steel block name {name:?}"),
            Self::UnknownBlock(name) => write!(formatter, "Steel block {name:?} is not registered"),
            Self::InvalidProperties(message) => formatter.write_str(message),
            Self::UnknownState(state_id) => write!(formatter, "Steel state id {state_id} is not registered"),
        }
    }
}

impl Error for BlockStateDescriptorError {}

/// Resolve a semantic descriptor into Steel's canonical state ID.
///
/// The Steel registry supplies omitted properties from its registered default
/// state. Call [`descriptor_from_steel`] to obtain the complete canonical
/// descriptor for the resulting state.
pub fn resolve_steel(
    descriptor: &BlockStateDescriptor,
) -> Result<BlockStateId, BlockStateDescriptorError> {
    let identifier: Identifier = descriptor
        .name()
        .parse()
        .map_err(|_| BlockStateDescriptorError::InvalidBlockName(descriptor.name().to_owned()))?;
    let block = REGISTRY
        .blocks
        .by_key(&identifier)
        .ok_or_else(|| BlockStateDescriptorError::UnknownBlock(descriptor.name().to_owned()))?;
    let properties = descriptor
        .properties()
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()));
    REGISTRY
        .blocks
        .state_id_from_block_defaulted_properties(block, properties)
        .ok_or_else(|| {
            BlockStateDescriptorError::InvalidProperties(format!(
                "invalid properties for Steel block {:?}",
                descriptor.name()
            ))
        })
}

/// Convert a Steel state ID into the complete semantic descriptor for that state.
pub fn descriptor_from_steel(
    state_id: BlockStateId,
) -> Result<BlockStateDescriptor, BlockStateDescriptorError> {
    let block = REGISTRY
        .blocks
        .by_state_id(state_id)
        .ok_or(BlockStateDescriptorError::UnknownState(state_id.0))?;
    let properties: BTreeMap<_, _> = REGISTRY
        .blocks
        .get_properties(state_id)
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    BlockStateDescriptor::new(block.key.to_string(), properties).map_err(|error| {
        BlockStateDescriptorError::InvalidProperties(format!(
            "Steel registry emitted an invalid descriptor: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use azalea::registry::builtin::BlockKind;
    use parkour_bot_azalea::BlockStateDescriptor;

    use super::{descriptor_from_steel, resolve_steel};
    use crate::registry::test_support::init_test_registry;

    #[test]
    fn stone_round_trips_between_azalea_descriptor_and_steel() {
        init_test_registry();
        let azalea_state: azalea::block::BlockState = BlockKind::Stone.into();
        let descriptor = BlockStateDescriptor::from_azalea(&azalea_state).unwrap();
        let steel_state = resolve_steel(&descriptor).unwrap();
        let steel_descriptor = descriptor_from_steel(steel_state).unwrap();
        assert_eq!(steel_descriptor, descriptor);

        let round_trip_azalea = steel_descriptor.resolve_azalea().unwrap();
        assert_eq!(
            BlockStateDescriptor::from_azalea(&round_trip_azalea).unwrap(),
            descriptor
        );
    }

    #[test]
    fn complete_stairs_properties_round_trip_in_both_registries() {
        init_test_registry();
        let descriptor = "minecraft:oak_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]"
            .parse::<BlockStateDescriptor>()
            .unwrap();
        let azalea_state = descriptor.resolve_azalea().unwrap();
        let azalea_descriptor = BlockStateDescriptor::from_azalea(&azalea_state).unwrap();
        assert_eq!(azalea_descriptor, descriptor);

        let steel_state = resolve_steel(&azalea_descriptor).unwrap();
        assert_eq!(descriptor_from_steel(steel_state).unwrap(), descriptor);
    }
}
