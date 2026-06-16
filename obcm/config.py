import json

# 0xFF is reserved as the end-of-features sentinel in chunk payloads, so style
# IDs occupy 1..254. ID 0 is left unused to keep "0 means unset" intuitive.
MAX_STYLE_ID = 254


def assign_style_ids(config: dict) -> dict:
    """Assign a unique style ID to every feature type, in place.

    Style IDs are a purely internal serialization detail (a uint8 reference into
    the per-file Style Table); no consumer depends on a specific value, only on
    global uniqueness. We therefore ignore any ``id`` present in the config and
    number features deterministically in document order, which makes collisions
    impossible by construction.
    """
    features = config.get("features", {})
    next_id = 1
    for feature_type in features.values():
        for style in feature_type.values():
            if next_id > MAX_STYLE_ID:
                raise ValueError(
                    f"Too many feature types: the style table supports at most "
                    f"{MAX_STYLE_ID} entries."
                )
            style["id"] = next_id
            next_id += 1
    return config


def load_config(path: str) -> dict:
    with open(path, 'r') as f:
        config = json.load(f)
    return assign_style_ids(config)
