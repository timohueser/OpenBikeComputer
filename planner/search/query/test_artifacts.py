import json
import pytest
import schema
from artifacts import check_labels, fingerprints, label_contract, model_identity


def test_contract_json_is_generated_from_the_schema():
    assert schema.CONTRACT.read_text() == schema.contract_json(), 'Run python3 query/schema.py'


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


def test_runtime_identity_names_the_opened_model_files(tmp_path):
    names = ('labels.json', 'tokenizer.json', 'model.int8.onnx')
    for name in names:
        (tmp_path / name).write_bytes(name.encode())
    before = model_identity(tmp_path)
    assert set(before) == set(names)
    (tmp_path / 'model.int8.onnx').write_bytes(b'changed')
    assert model_identity(tmp_path)['model.int8.onnx'] != before['model.int8.onnx']
    (tmp_path / 'tokenizer.json').unlink()
    with pytest.raises(FileNotFoundError):
        model_identity(tmp_path)
