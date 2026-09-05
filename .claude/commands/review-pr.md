Review pull request #$ARGUMENTS

Fetch the PR diff and details using `gh pr view` and `gh pr diff`, then analyze for:

1. **Code Quality**
   - Rust idioms and Substrate/Polkadot SDK patterns
   - Error handling and unwrap usage
   - Code clarity and maintainability
   - No useless comments (explain "why" not "how")

2. **Security**
   - Unsafe code blocks
   - Input validation
   - Access control in pallets
   - No panics in runtime code
   - Bounded collections usage

3. **Performance**
   - Weight/benchmark implications
   - Storage access patterns
   - Unnecessary allocations
   - Arithmetic safety (checked_*, saturating_*)

4. **Testing**
   - Test coverage for new code
   - Edge cases handled
   - Unit tests for pallet changes
   - Integration tests for complex features

5. **Breaking Changes**
   - API compatibility
   - Migration requirements
   - Extrinsic signature changes

6. **FRAME Pallet Standards**
   - Appropriate storage types
   - Events for state changes
   - Descriptive error types
   - Accurate weight annotations

Provide specific feedback with file paths and line numbers.
