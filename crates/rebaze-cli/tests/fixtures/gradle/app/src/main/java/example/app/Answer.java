package example.app;
import example.core.Core;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
public class Answer {
    public static boolean correct() throws Exception {
        Class.forName("org.apache.commons.lang3.StringUtils");
        try (InputStream value = Answer.class.getResourceAsStream("/value.txt")) {
            int number = Integer.parseInt(new String(value.readAllBytes(), StandardCharsets.UTF_8).trim());
            return Core.matcher().matches(number);
        }
    }
}
