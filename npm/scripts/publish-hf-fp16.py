#!/usr/bin/env python3
"""Publish the fp16 `multilingual-e5-small` to a HuggingFace repo you own, so
hf-mirror.com mirrors it automatically and BOTH international (huggingface.co)
and mainland-China (hf-mirror.com) users download the SAME smaller ~224 MB fp16
model — consistently, instead of some getting the 448 MB f32 original.

What it does:
  1. download config.json + tokenizer.json + model.safetensors (f32) from the
     upstream `intfloat/multilingual-e5-small`
  2. quantize model.safetensors f32 -> f16 (~448 MB -> ~224 MB; the Rust binary
     casts back to f32 on load, so retrieval quality is unchanged)
  3. create your repo (if it doesn't exist) and upload the three files

Prereqs:
  pip install -U huggingface_hub safetensors numpy
  a (free) HuggingFace account + a WRITE access token:
      https://huggingface.co/settings/tokens   (New token -> type "Write")

Usage:
  HF_TOKEN=hf_xxx python3 npm/scripts/publish-hf-fp16.py <username-or-org>/<repo-name>

Example:
  HF_TOKEN=hf_xxx python3 npm/scripts/publish-hf-fp16.py umacloud/umadev-embed-e5-small-fp16
"""
import os
import sys
import tempfile

UPSTREAM = "intfloat/multilingual-e5-small"


def main() -> None:
    if len(sys.argv) != 2 or "/" not in sys.argv[1]:
        sys.exit("usage: publish-hf-fp16.py <username-or-org>/<repo-name>")
    repo_id = sys.argv[1]
    token = os.environ.get("HF_TOKEN") or os.environ.get("HUGGINGFACE_TOKEN")
    if not token:
        sys.exit(
            "set HF_TOKEN to a HuggingFace WRITE token "
            "(https://huggingface.co/settings/tokens)"
        )

    try:
        from huggingface_hub import hf_hub_download, create_repo, upload_file
        from safetensors.numpy import load_file, save_file
        import numpy as np
    except ImportError:
        sys.exit("missing deps — run: pip install -U huggingface_hub safetensors numpy")

    work = tempfile.mkdtemp(prefix="umadev-hf-")

    print(f"[1/3] downloading upstream {UPSTREAM} …")
    local = {
        f: hf_hub_download(UPSTREAM, f, local_dir=work)
        for f in ("config.json", "tokenizer.json", "model.safetensors")
    }

    print("[2/3] quantizing model.safetensors  f32 -> f16  (~448MB -> ~224MB) …")
    tensors = load_file(local["model.safetensors"])
    fp16 = {
        k: (v.astype(np.float16) if v.dtype == np.float32 else v)
        for k, v in tensors.items()
    }
    fp16_path = os.path.join(work, "model.fp16.safetensors")
    save_file(fp16, fp16_path)

    print(f"[3/3] creating repo {repo_id} and uploading …")
    create_repo(repo_id, token=token, repo_type="model", exist_ok=True)
    for src, name in (
        (local["config.json"], "config.json"),
        (local["tokenizer.json"], "tokenizer.json"),
        (fp16_path, "model.safetensors"),
    ):
        print(f"      uploading {name} …")
        upload_file(
            path_or_fileobj=src,
            path_in_repo=name,
            repo_id=repo_id,
            token=token,
            repo_type="model",
        )

    print("\n[ok] done. Your fp16 model is live at:")
    print(f"   https://huggingface.co/{repo_id}")
    print("   files: config.json · tokenizer.json · model.safetensors (fp16, ~224MB)")
    print(f"\nNext: tell the maintainer the repo id  '{repo_id}'  and cli.js will point")
    print(f"   at it — huggingface.co/{repo_id} for international, hf-mirror.com/{repo_id}")
    print("   for China — so everyone downloads the SAME 224MB fp16.")


if __name__ == "__main__":
    main()
