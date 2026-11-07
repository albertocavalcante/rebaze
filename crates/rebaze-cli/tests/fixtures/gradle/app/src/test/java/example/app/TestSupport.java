package example.app;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
public class TestSupport {
    // @Test in a comment must not create a Bazel test target.
    public static boolean correct() throws Exception {
        try (InputStream input = TestSupport.class.getResourceAsStream("/test-value.txt")) {
            return new String(input.readAllBytes(), StandardCharsets.UTF_8).trim().equals("test resource: 42") && Answer.correct();
        }
    }
}
