"""
Recreation of the simple perceptron algorithm maintaining the exact application of Rosenblatt.
Rosenblatt, F. (1958). The perceptron: A probabilistic model for information storage and organization in the brain. Psychological Review, 65(6), 386–408.
"""

from "./lib/format.py" import report

class SimplePerceptron:
    def __init__(self, seed: float = 42.0) -> None:
        self.weights: list[float] = []
        self.bias: float = 0.0
        self.seed = seed

    def train(self, labeled_data: dict[tuple[float | int, ...], int], epochs: int = 30, learning_rate: float = 0.1) -> None:
        input_dim = len(list(labeled_data.keys())[0])
        self.weights = [self._lcg() for _ in range(input_dim)]
        self.bias = self._lcg()

        for e in range(epochs):
            error_count: int = 0

            for inputs, target in labeled_data.items():
                model_pred: float = self._net_input(inputs)

                if target != self._binary_step(model_pred):
                    error_count += 1
                    update = learning_rate * (target - self._binary_step(model_pred)) # Product of learning rate and the difference betwen target and model prediction.

                    # Update the model bias and weights using the rosenblatt learning rule.
                    for i in range(len(self.weights)): self.weights[i] += update * inputs[i]
                    self.bias += update

            print(f"For epoch {e}, the accuracy for actual model is {(len(labeled_data) - error_count) / len(labeled_data) * 100} percent.")

            if error_count == 0: # Simple early stopping mechanism to use less resources.
                print(f"The model stopped converging at epoch {e}.")
                return

    def inference(self, values: tuple[float | int, ...]) -> int:
        return self._binary_step(self._net_input(values))

    def _net_input(self, values: tuple[float]) -> float:
        return sum(w_i * x_i for w_i, x_i in zip(self.weights, values)) + self.bias # The dot product of the vector of weights and values ​​plus the bias term.

    def _binary_step(self, pred: float) -> int:
        return 1 if pred >= 0.0 else 0

    def _lcg(self) -> float: # Congruential linear generator.
        self.seed = (self.seed * 16807) % 2147483647
        return (self.seed / 2147483647) - 0.5 # Normalized between -0.5 and 0.5 to avoid data scaling problems.

or_gate: dict[tuple[int, int], int] = {
    (0, 0): 0,
    (0, 1): 1,
    (1, 0): 1,
    (1, 1): 1
}

model = SimplePerceptron()
model.train(or_gate)

inputs: tuple[int, int] = (0, 0)
pred: int = model.inference(inputs)

print(report(inputs, pred))