# Semantic-mutant manifest

The project maintains a fixed suite of semantic mutations — deliberate
behavior-changing edits — that the test suite must kill. This manifest
maps each mutation to the public tests that fail
when it is introduced. Test names are `<file> :: <function>` under
`tests/`.

| # | Mutation | Killing tests |
| --- | --- | --- |
| 1 | accept a width outside the Width Allowlist | `request_tests::widths_outside_the_allowlist_are_rejected`, `e2e_tests::e2e_invalid_requests_never_reach_the_fixture_server` |
| 2 | consult one global quality allowlist instead of the selected format policy | `request_tests::quality_allowlists_are_per_format`, `request_tests::qualities_outside_the_selected_policy_are_rejected`, `request_tests::empty_quality_allowlist_permits_no_explicit_quality` |
| 3 | accept an explicit quality equal to the selected format's default | `request_tests::quality_equal_to_the_format_default_is_rejected` |
| 4 | require `v`, or mark an unversioned response immutable | `request_tests::missing_v_is_accepted`, `response_tests::unversioned_success_uses_the_short_ttl_and_is_never_immutable`, `e2e_tests::e2e_omitting_v_returns_the_same_pixels_with_the_shorter_policy` |
| 5 | append `v` to an HTTP(S) path or S3 object key | `adapter_http_tests::success_returns_bytes_and_builds_percent_encoded_path` (asserts observed path), `adapter_s3_tests::success_observes_the_exact_key_without_any_version`, `e2e_tests::e2e_s3_source_serves_the_same_contract_and_key_excludes_v` |
| 6 | permit upscaling by reversing the width comparison | `image_tests::narrow_source_is_never_upscaled`, `e2e_tests::e2e_source_narrower_than_the_target_is_not_enlarged` |
| 7 | map Source denial to 404 | `adapter_http_tests::statuses_403_500_503_map_to_unavailable`, `adapter_s3_tests::access_denied_maps_to_unavailable_and_never_not_found`, `response_tests::permission_denial_is_a_502_and_cannot_be_mistaken_for_absence`, `e2e_tests::e2e_denied_source_returns_a_non_cacheable_502` |
| 8 | cache a 5xx response | `response_tests::upstream_500_is_a_non_cacheable_502`, `response_tests::source_timeout_is_a_non_cacheable_504`, `response_tests::undecodable_source_bytes_are_a_non_cacheable_502`, e2e items 6-8 (each asserts `Cache-Control: no-store`) |
| 9 | remove the streamed-body size check while keeping the `Content-Length` check | `adapter_http_tests::streamed_body_without_content_length_over_limit_is_too_large`, `adapter_http_tests::huge_chunked_body_without_content_length_is_too_large`, `adapter_s3_tests::streamed_body_over_limit_without_content_length_is_too_large`, `adapter_fs_tests::oversized_file_is_too_large` (fs has no advertised header path) |
| 10 | remove encoded traversal validation | `request_tests` traversal cases (literal, `%2E`, `%252E`, encoded-delimiter forms), `e2e_tests::e2e_traversal_cannot_read_a_sentinel_outside_the_filesystem_root` |
| 11 | allow a redirect outside the configured origin or base path | `adapter_http_tests::redirect_to_a_different_port_host_or_scheme_is_unavailable`, `adapter_http_tests::redirect_escaping_the_base_path_is_unavailable`, `adapter_http_tests::redirect_chain_exceeding_the_limit_is_unavailable` |
| 12 | allow a filesystem symlink | `adapter_fs_tests::symlink_to_file_inside_root_is_rejected`, `adapter_fs_tests::symlink_to_file_outside_root_is_rejected`, `adapter_fs_tests::symlinked_intermediate_directory_is_rejected` |
| 13 | flatten alpha for WebP or fail to flatten it for JPEG | `image_tests::transparency_survives_webp`, `image_tests::transparency_survives_avif`, `image_tests::transparent_source_flattens_to_white_for_jpeg` |
| 14 | ignore `If-None-Match` and always re-derive | `response_tests::matching_if_none_match_is_304_without_a_body`, `response_tests::http_origin_etag_revalidates_via_head` |
| 15 | emit a strong derived ETag from a weak upstream tag | `response_tests::weak_upstream_etag_stays_weak_on_the_derived_tag` |
| 16 | treat an identify failure (other than timeout) as the client answer | `response_tests::identify_head_403_falls_through_to_get`, `response_tests::identify_head_404_falls_through_to_get` |
| 17 | acquire a derivation permit before identify | `response_tests::matching_if_none_match_skips_saturated_derivation_permits` |

## Documented equivalent mutants

- `src/processor.rs`: the overflow arm of `width.checked_mul(height)` for
  two `i32`-sourced header dimensions cannot fire (their product always
  fits in `u64`); the checked-arithmetic contract is exercised instead
  through the limit-side overflow (`max_source_megapixels * 1_000_000`
  with `u64::MAX`) and the huge-header PNG test.
