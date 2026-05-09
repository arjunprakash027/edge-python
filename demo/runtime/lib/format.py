"""
Output formatting helpers for the perceptron demo. Keeps the model code
focused on the algorithm and the presentation isolated here.
"""

def label(prediction: int) -> str:
    return "active" if prediction == 1 else "inactive"

def report(inputs: tuple, prediction: int) -> str:
    return f"inputs={inputs} -> prediction={prediction} ({label(prediction)})"

if __name__ == "__main__":
    # Standalone smoke check — only runs when this file is the entry script.
    # When imported, the parser inlines `label` and `report` and ignores this.
    print(report((0, 1), 1))
    print(report((0, 0), 0))