"""Write gold labels (gold.json) for the ARC-Easy / HellaSwag items run.py scores."""
import json
from datasets import load_dataset
arc = load_dataset("allenai/ai2_arc", "ARC-Easy", split="test")
hs = load_dataset("Rowan/hellaswag", split="validation")
json.dump({"arc": [ex["choices"]["label"].index(ex["answerKey"]) for ex in arc.select(range(500))],
           "hs": [int(x) for x in hs.select(range(300))["label"]]}, open("gold.json", "w"))
