import json
import pytest
from artifacts import check_labels, fingerprints, label_contract


def test_label_order_is_part_of_the_model_contract(tmp_path):
    labels = label_contract()
    path = tmp_path / 'labels.json'
    path.write_text(json.dumps(labels))
    check_labels(tmp_path)
    labels['intents'] = list(reversed(labels['intents']))
    path.write_text(json.dumps(labels))
    with pytest.raises(ValueError, match='Model labels'):
        check_labels(tmp_path)
    path.unlink()
    with pytest.raises(FileNotFoundError):
        check_labels(tmp_path)


def test_training_input_fingerprint_changes_with_content(tmp_path):
    path = tmp_path / 'input.jsonl'
    path.write_text('one')
    before = fingerprints([path])
    path.write_text('two')
    assert fingerprints([path]) != before
