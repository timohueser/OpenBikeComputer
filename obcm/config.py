import json

def load_config(path: str) -> dict:
    with open(path, 'r') as f:
        return json.load(f)
