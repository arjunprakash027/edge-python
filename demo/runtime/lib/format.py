"""
Inheritance and dunder methods applied to report classes formatting perceptron prediction outputs.
"""

class Report:
    def __init__(self, inputs: tuple, prediction: int) -> None:
        self.inputs = inputs
        self.prediction = prediction

    def _label(self) -> str:
        return "active" if self.prediction == 1 else "inactive"
    
    def __str__(self) -> str:
        # Called automatically by print() and str().
        return f"inputs={self.inputs} -> prediction={self.prediction} ({self._label()})"

class DictReport(Report):
    # Inherits __init__, _label and __str__ from Report; adds to_dict().
    def to_dict(self) -> dict[str, tuple[int, int] | int]:
        return {
            "inputs": list(self.inputs),
            "prediction": self.prediction,
            "label": self._label(),
        }

if __name__ == "__main__":
    print(Report((0, 1), 1))
    print(DictReport((0, 1), 1).to_dict())
