# Semantic-mutant manifest

SPEC.md defines a fixed suite of semantic mutations that the test suite
must kill. This manifest maps each mutation to the public tests that fail
when it is introduced. Test names are `<file> :: <function>` under
`tests/`.

| # | Mutation | Killing tests |
| --- | --- | --- |
| 1 | accept a width outside the Width Allowlist | `request_tests::widths_outside_the_allowlist_are_rejected`, `e2e_tests` invalid-request acceptance (no fixture I/O) |
| 2 | consult one global quality allowlist instead of the selected format policy | `request_tests::quality_allowlists_are_per_format`, `request_tests::qualities_outside_the_selected_policy_are_rejected`, `request_tests::empty_quality_allowlist_permits_no_explicit_quality` |
| 3 | accept an explicit quality equal to the selected format's default | `request_tests::quality_equal_to_the_format_default_is_rejected` |
| 4 | require `v`, or mark an unversioned response immutable | `request_tests::missing_v_is_accepted`, `response_tests` unversioned cache-policy tests, e2e acceptance item 12 |
| 5 | append `v` to an HTTP(S) path or S3 object key | `adapter_http_tests::success_returns_bytes_and_builds_percent_encoded_path` (asserts observed path), `adapter_s3_tests::success_observes_the_exact_key_without_any_version`, `adapter_s3_tests::assert_no_version_in_query` (helper used by every S3 success test), e2e S3 acceptance |
| 6 | permit upscaling by reversing the width comparison | `image_tests::narrow_source_is_never_upscaled`, e2e acceptance item 4 |
| 7 | map Source denial to 404 | `adapter_http_tests::statuses_403_500_503_map_to_unavailable`, `adapter_s3_tests::access_denied_maps_to_unavailable_and_never_not_found`, `response_tests` denial tests, e2e acceptance item 6 |
| 8 | cache a 5xx response | `response_tests` no-store tests, e2e acceptance items 6-8 (assert `Cache-Control: no-store`) |
| 9 | remove the streamed-body size check while keeping the `Content-Length` check | `adapter_http_tests::streamed_body_without_content_length_over_limit_is_too_large`, `adapter_http_tests::huge_chunked_body_without_content_length_is_too_large`, `adapter_s3_tests::streamed_body_over_limit_without_content_length_is_too_large`, `adapter_fs_tests::oversized_file_is_too_large` (fs has no advertised header path) |
| 10 | remove encoded traversal validation | `request_tests` traversal cases in `lowercase_and_invalid_percent_triplets_are_rejected` / `percent_encoded_unreserved_ascii_is_rejected_generated` / structure tests covering `.`, `..`, `%2E`, `%252E` forms, e2e acceptance item 10 (sentinel outside root) |
| 11 | allow a redirect outside the configured origin or base path | `adapter_http_tests::redirect_to_a_different_port_host_or_scheme_is_unavailable`, `adapter_http_tests::redirect_escaping_the_base_path_is_unavailable`, `adapter_http_tests::redirect_chain_exceeding_the_limit_is_unavailable` |
| 12 | allow a filesystem symlink | `adapter_fs_tests::symlink_to_file_inside_root_is_rejected`, `adapter_fs_tests::symlink_to_file_outside_root_is_rejected`, `adapter_fs_tests::symlinked_intermediate_directory_is_rejected` |
| 13 | flatten alpha for WebP or fail to flatten it for JPEG | `image_tests::transparency_survives_webp`, `image_tests::transparency_survives_avif`, `image_tests::transparent_source_flattens_to_white_for_jpeg` |

## Documented equivalent mutants

- `src/processor.rs`: the overflow arm of `width.checked_mul(height)` for
  two `i32`-sourced header dimensions cannot fire (their product always
  fits in `u64`); the checked-arithmetic contract is exercised instead
  through the limit-side overflow (`max_source_megapixels * 1_000_000`
  with `u64::MAX`) and the huge-header PNG test.
- `src/processor.rs`: `ProcessError::Resize` is structurally unreachable in
  the fused-thumbnail design (decode and resize failures surface before a
  valid source is "accepted" and are classified `Undecodable`); the variant
  exists because the spec's outcome set requires `resize_failed` to be
  representable.
