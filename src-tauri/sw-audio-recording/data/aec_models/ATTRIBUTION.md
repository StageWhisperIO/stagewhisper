# Acoustic Echo Cancellation models

`model_128_1.onnx` and `model_128_2.onnx` are the DTLN-AEC (size 128) two-stage
models from the fastrepl/anarlog project, used under the MIT License.

Source: https://github.com/fastrepl/anarlog (crates/aec/data/models)

The Rust inference code in `src/aec/` is a port of fastrepl/anarlog `crates/aec`
(MIT License), adapted to call `ort` directly instead of their `hypr-onnx` wrapper.

The MIT License copyright notice and permission notice below cover both the
bundled ONNX models and the ported inference code, as required when
redistributing copies or substantial portions of MIT-licensed software.

---

MIT License

Copyright (c) 2023-present Fastrepl, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
