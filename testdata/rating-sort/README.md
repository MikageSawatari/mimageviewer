# Rating sort UI smoke fixture

The three 8×8 PNGs are disposable test inputs. `01-one.png` starts at ★1,
`03-two.png` starts at ★2, and `02-unrated.png` has no rating. The UI smoke
runner copies them into `target/portable-smoke/data/rating-sort/fixture` and
seeds ratings in that disposable data directory. The source files contain no
user data.

Regenerate the images with:

```powershell
python scripts/ui-smoke/generate_rating_sort_fixture.py testdata/rating-sort
```
